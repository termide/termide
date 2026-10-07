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
use termide_agent_core::recap;
use termide_agent_core::{
    companion_tools, parse_verdict, GoalPrompt, HandoffPrompt, IntentLog, ModelSpec, Provider,
    ReviewerSetup, SessionView, ThinkingLevel,
};
use termide_agent_core::{
    expand_env, now_millis, AcpConfig, AcpFlavor, Agent, AgentCommand, AgentEvent,
    AssistantContent, AssistantMessage, Backend, BackendModel, BackendOption, BackendSetup,
    CancelToken, ExternalSessionRef, Hooks, HostTools, Message, Mode, ModeHandle, PermissionHooks,
    PlanPrompt, PromptError, StopReason, StreamEvent, ToolCall, ToolContext, ToolDecision,
    ToolRegistry, ToolResultMessage, Usage, UserMessage, ACP_PROVIDER, RECAP_LIMIT,
};
use termide_agent_mcp::{McpServer, SERVER_NAME};

mod provider;
pub use provider::AcpProvider;

/// How the calls of termide's tools reach the transcript from Claude Code:
/// named after the MCP server that serves them.
const HOST_TOOL_PREFIX: &str = "mcp__termide__";

/// The protocol version requested.
pub const PROTOCOL_VERSION: u64 = 1;

/// How long Codex and Gemini CLI wait to open their session for termide's MCP
/// servers to answer: they list the tools of an MCP server once, when the
/// session starts, so tools that come later never reach them.
const MCP_SETTLE: Duration = Duration::from_secs(10);

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;
type SharedWriter = Arc<Mutex<Option<Box<dyn Write + Send>>>>;

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
    /// The server of termide's tools, alive as long as the agent.
    mcp_server: Mutex<Option<McpServer>>,
    /// Whether termide's MCP servers have all answered, which Codex and
    /// Gemini CLI wait on (for [`MCP_SETTLE`] at most) to open their session.
    mcp_settled: Mutex<bool>,
    mcp_settled_changed: Condvar,
    /// Calls announced without their arguments yet, by id.
    announced: Mutex<HashMap<String, Value>>,
    /// The running calls of termide's own tools, by id, with the tool's name
    /// and arguments: the agent may still ask for their permission, which
    /// termide judges on its server, their end may not name the tool again,
    /// and the details the server kept for the UI are found by them.
    host_calls: Mutex<HashMap<String, (String, Value)>>,
    /// The calls on their way into the session log.
    calls: Mutex<CallLog>,
    /// The context's fill and size, `(used, size)`, as `usage_update` last
    /// reported them.
    context: Mutex<Option<(u64, u64)>>,
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
    /// The recap that goes before the first request, see [`recap`].
    recap: Mutex<Option<String>>,
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
    /// The side request in flight: the session it runs in and the answer
    /// so far, which reaches neither the panel nor the log.
    side: Mutex<HashMap<String, String>>,
    goal: GoalPrompt,
    handoff: HandoffPrompt,
}

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
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(&setup.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &config.env {
            command.env(key, expand_env(value, |var| std::env::var(var).ok()));
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", config.command))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            let agent = name.to_string();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!("acp {agent}: {line}");
                }
            });
        }
        let cwd = setup.cwd.clone();
        let runtime = Self::connect(
            name,
            stdout,
            stdin,
            setup,
            config.timeout_secs,
            config.flavor,
            service,
        );
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
        Self::connect(name, reader, writer, setup, timeout_secs, flavor, false)
    }

    /// [`Self::from_streams`], as a service when `service` says so.
    fn connect(
        name: &str,
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        setup: BackendSetup,
        timeout_secs: u64,
        flavor: AcpFlavor,
        service: bool,
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
            models: Mutex::new(Vec::new()),
            current_model: Mutex::new(None),
            model_option: Mutex::new(None),
            options: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
            flavor,
            system_prompt: setup.system_prompt,
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
            mcp_settled: Mutex::new(false),
            mcp_settled_changed: Condvar::new(),
            announced: Mutex::new(HashMap::new()),
            host_calls: Mutex::new(HashMap::new()),
            calls: Mutex::new(CallLog::default()),
            context: Mutex::new(None),
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
            recap: Mutex::new(None),
            replaying: AtomicBool::new(false),
            steering: AtomicBool::new(false),
            turn_running: AtomicBool::new(false),
            steered: Mutex::new(Vec::new()),
            steering_out: Mutex::new(HashMap::new()),
            service,
            can_fork: AtomicBool::new(false),
            can_close: AtomicBool::new(false),
            side: Mutex::new(HashMap::new()),
            goal: setup.goal,
            handoff: setup.handoff,
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
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(0, message);
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
        if self.shared.steering.load(Ordering::Acquire)
            && self.shared.turn_running.load(Ordering::Acquire)
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
            queued.saturating_sub(usize::from(!running && queued > 0)),
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
    }

    fn is_busy(&self) -> bool {
        self.shared.busy.load(Ordering::Acquire)
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
            let event = match shared.side_query(&text) {
                Ok(reply) if !reply.trim().is_empty() => {
                    let verdict = parse_verdict(&reply);
                    AgentEvent::GoalJudged {
                        done: verdict.done,
                        reason: verdict.reason,
                    }
                }
                Ok(_) => AgentEvent::GoalJudgeFailed {
                    error: "the judge call did not complete".to_string(),
                },
                Err(error) => AgentEvent::GoalJudgeFailed { error },
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
            let brief = match shared.side_query(&text) {
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
                    "terminal": false
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
                Err(error) => log::warn!(
                    "acp {}: cannot resume session {}: {error}",
                    self.name,
                    resume.session_id
                ),
            }
        }
        let value = self.request("session/new", params, timeout)?;
        if let Some(id) = value["sessionId"].as_str() {
            let _ = self.events.send(AgentEvent::ExternalSession {
                agent: self.name.clone(),
                session_id: id.to_string(),
            });
            *self.recap.lock().unwrap_or_else(PoisonError::into_inner) =
                recap(&history, RECAP_LIMIT);
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
        let mut options = json!({ "settingSources": [], "strictMcpConfig": true });
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
        // Everything queued goes as one turn: messages typed while the agent
        // works are usually one thought written in pieces.
        let message = UserMessage::merge(
            self.queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain(..)
                .collect(),
        );
        let Some(message) = message else {
            let _ = self.events.send(AgentEvent::AgentEnd);
            self.busy.store(false, Ordering::Release);
            // A message a turn did not take back after all (see
            // [`Self::requeue`]) may have arrived meanwhile: it runs now.
            self.run_queued();
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
        let text = message.plain_text();
        let mut prompt = Vec::new();
        // Kept out of the log, like the plan note: the log holds the
        // conversation itself.
        if let Some(recap) = self
            .recap
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            prompt.push(json!({ "type": "text", "text": recap }));
        }
        for note in [self.plan_switch_note(), self.tools_note()]
            .into_iter()
            .flatten()
        {
            prompt.push(json!({ "type": "text", "text": note }));
        }
        prompt.push(json!({ "type": "text", "text": text }));
        self.turn_running.store(true, Ordering::Release);
        let result = self.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": prompt
            }),
            Duration::from_secs(60 * 60 * 24),
        );
        self.turn_running.store(false, Ordering::Release);
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
            Err(error) => Some(error.clone()),
            Ok(_) if stop == StopReason::Aborted => Some("aborted".to_string()),
            Ok(_) => None,
        };
        let usage = result.as_ref().map(usage_of).unwrap_or_default();
        self.close_unfinished_calls();
        self.close_message_with(stop, error, usage);
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
            steering: queued,
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
            id: None,
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
        self.session_id()
            .map(|_| ())
            .map_err(|_| PromptError::Stopped)
    }

    /// Ask the agent `text` aside from the conversation and return its
    /// answer's text: in a fork of the session when the agent forks one,
    /// closed afterwards when it closes one; else in the session itself, as
    /// a turn kept from the panel and the log (the agent's own session still
    /// holds it). Its tools are not asked for: a permission request from it
    /// is refused.
    fn side_query(self: &Arc<Self>, text: &str) -> Result<String, String> {
        let main = self.session_id()?;
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
            return Err("the agent is busy".to_string());
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
    /// answer's text, which reaches neither the panel nor the log; several
    /// may run at once, each in its own session.
    fn side_prompt(
        &self,
        session_id: &str,
        text: &str,
        timeout: Duration,
    ) -> Result<String, String> {
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
        let reply = result?;
        match reply["stopReason"].as_str() {
            Some(stop @ ("cancelled" | "refusal")) => Err(format!("the agent stopped: {stop}")),
            _ => Ok(answer),
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
        let text = message.plain_text();
        self.steering_out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, message);
        let request = json!({
            "jsonrpc": "2.0", "id": id, "method": "_session/steering",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": text }],
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

    /// Tell the panel how many messages wait.
    fn report_queue(&self) {
        let queued = self
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        let running = self.busy.load(Ordering::Acquire);
        // While the session is starting, the first message is the run's own.
        let steering = queued.saturating_sub(usize::from(!running && queued > 0));
        let _ = self.events.send(AgentEvent::QueueUpdate {
            steering,
            follow_up: 0,
        });
    }

    /// Finish the assistant message being streamed, or make one for an
    /// error, so the transcript and the log get a complete message.
    fn close_message(&self, stop: StopReason, error: Option<String>) {
        self.close_message_with(stop, error, Usage::default());
    }

    /// [`Self::close_message`], with the turn's token usage when the agent
    /// reported it.
    fn close_message_with(&self, stop: StopReason, error: Option<String>, usage: Usage) {
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
            timestamp: now_millis(),
        };
        let _ = self
            .events
            .send(AgentEvent::MessageEnd(Message::Assistant(message)));
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
            return Err(error);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                return Err(format!("{method}: no reply within {} s", timeout.as_secs()));
            }
            match rx.recv_timeout(left.min(Duration::from_millis(100))) {
                Ok(reply) => return reply.map_err(|error| format!("{method}: {error}")),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("{method}: the agent closed the connection"))
                }
            }
        }
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
                        Some(error) => Err(format!(
                            "{} (code {})",
                            error["message"].as_str().unwrap_or("error"),
                            error["code"]
                        )),
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
        let call = permission_call(&params["toolCall"]);
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
    fn on_update(&self, update: &Value) {
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
            )
        {
            return;
        }
        match kind {
            "agent_message_chunk" => {
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
                    }
                }
                if is_finished(update) {
                    self.finish_tool_call(update);
                }
            }
            "usage_update" => {
                if let (Some(used), Some(size)) = (update["used"].as_u64(), update["size"].as_u64())
                {
                    *self.context.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some((used, size));
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
            let result =
                ToolResultMessage::error(&call, "The turn ended before this call finished.");
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
        // termide ran it: the details its tool gave, which the MCP reply
        // leaves out, so its block draws as for the built-in agent.
        let host_details = host.and_then(|(name, arguments)| {
            call.name = name;
            self.mcp_server
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()?
                .take_details(&call.name, &arguments)
        });
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
            ToolResultMessage::text(&call, text)
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
    let kind = update["kind"].as_str().unwrap_or("other").to_string();
    if !title.is_empty() {
        arguments["title"] = json!(title);
    }
    ToolCall {
        id: update["toolCallId"].as_str().unwrap_or("").to_string(),
        name: kind,
        arguments,
        extra_content: None,
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
fn usage_of(result: &Value) -> Usage {
    let usage = &result["usage"];
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    Usage {
        input: count("inputTokens"),
        output: count("outputTokens"),
        cache_read: count("cachedReadTokens"),
        cache_write: count("cachedWriteTokens"),
    }
}

/// Whether a tool call update carries the call's arguments.
fn has_arguments(update: &Value) -> bool {
    update["rawInput"]
        .as_object()
        .is_some_and(|input| !input.is_empty())
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
/// same logic judges it: `execute` becomes `bash` (the command placed under
/// `command`, from the raw input or the title), `read`/`edit` keep their
/// names, and anything else stands as its kind, which no built-in rule covers
/// and so reaches the user.
fn permission_call(tool_call: &Value) -> ToolCall {
    let title = tool_call["title"].as_str().unwrap_or("").to_string();
    let kind = tool_call["kind"].as_str().unwrap_or("other");
    let name = match kind {
        "execute" => "bash",
        other => other,
    };
    let mut arguments = tool_call
        .get("rawInput")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    // `bash` rules match on the command line; if the agent gave none in the
    // raw input, the title is the best stand-in for a read-only check.
    if name == "bash" && arguments.get("command").and_then(Value::as_str).is_none() {
        arguments["command"] = json!(title);
    }
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

fn read_text_file(cwd: &Path, params: &Value) -> Result<Value, String> {
    let path = absolute(cwd, params["path"].as_str().ok_or("path missing")?);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let line = params["line"].as_u64().map(|l| l.max(1) as usize - 1);
    let limit = params["limit"].as_u64().map(|l| l as usize);
    let content = match (line, limit) {
        (None, None) => text,
        (line, limit) => text
            .lines()
            .skip(line.unwrap_or(0))
            .take(limit.unwrap_or(usize::MAX))
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
    std::fs::write(&path, content)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::pipe;
    use termide_agent_core::{
        permission_channel, PermissionAnswer, PermissionEnvelope, PermissionRules,
    };

    /// An agent on the other end of two pipes, scripted for one prompt: it
    /// thinks, speaks, edits a file through our fs methods after asking for
    /// permission, speaks again and ends the turn; a second prompt is
    /// cancelled.
    fn fake_agent(dir: PathBuf) -> (AcpRuntime, Receiver<PermissionEnvelope>) {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        let agent_dir = dir.clone();
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut send = |value: Value| writeln!(out, "{value}").unwrap();
            let mut reader = BufReader::new(to_agent_rx).lines();
            let mut next_id = 100;
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                let id = message["id"].clone();
                match message["method"].as_str() {
                    Some("initialize") => {
                        assert_eq!(
                            message["params"]["clientCapabilities"]["fs"]["writeTextFile"],
                            true
                        );
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": 1, "agentCapabilities": {} } }),
                        );
                    }
                    Some("session/new") => {
                        assert_eq!(message["params"]["cwd"], json!(agent_dir));
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "sessionId": "s1" } }),
                        );
                    }
                    Some("session/prompt") => {
                        let text = message["params"]["prompt"][0]["text"]
                            .as_str()
                            .unwrap()
                            .to_string();
                        if text == "second" {
                            // Wait for the cancel notification, then stop.
                            loop {
                                let Some(Ok(line)) = reader.next() else {
                                    return;
                                };
                                let m: Value = serde_json::from_str(&line).unwrap();
                                if m["method"] == "session/cancel" {
                                    break;
                                }
                            }
                            send(
                                json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "cancelled" } }),
                            );
                            continue;
                        }
                        let update = |u: Value| json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s1", "update": u } });
                        send(update(
                            json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hmm" } }),
                        ));
                        send(update(
                            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Editing " } }),
                        ));
                        send(update(
                            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "now." } }),
                        ));
                        // Ask for permission and wait for the answer.
                        next_id += 1;
                        send(
                            json!({ "jsonrpc": "2.0", "id": next_id, "method": "session/request_permission", "params": {
                            "sessionId": "s1",
                            "toolCall": { "toolCallId": "t1", "title": "Write notes.md", "kind": "edit", "rawInput": { "path": "notes.md" } },
                            "options": [
                                { "optionId": "o-once", "name": "Allow", "kind": "allow_once" },
                                { "optionId": "o-always", "name": "Always", "kind": "allow_always" },
                                { "optionId": "o-no", "name": "Reject", "kind": "reject_once" }
                            ] } }),
                        );
                        let answer: Value = loop {
                            let Some(Ok(line)) = reader.next() else {
                                return;
                            };
                            let m: Value = serde_json::from_str(&line).unwrap();
                            if m["id"] == json!(next_id) {
                                break m;
                            }
                        };
                        assert_eq!(answer["result"]["outcome"]["optionId"], "o-once");
                        send(update(
                            json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Write notes.md", "kind": "edit", "status": "in_progress", "rawInput": { "path": "notes.md" } }),
                        ));
                        // Write through the client and read it back.
                        next_id += 1;
                        send(
                            json!({ "jsonrpc": "2.0", "id": next_id, "method": "fs/write_text_file", "params": { "sessionId": "s1", "path": "notes.md", "content": "one\ntwo\nthree\n" } }),
                        );
                        loop {
                            let Some(Ok(line)) = reader.next() else {
                                return;
                            };
                            let m: Value = serde_json::from_str(&line).unwrap();
                            if m["id"] == json!(next_id) {
                                assert!(m.get("error").is_none(), "{m}");
                                break;
                            }
                        }
                        next_id += 1;
                        send(
                            json!({ "jsonrpc": "2.0", "id": next_id, "method": "fs/read_text_file", "params": { "sessionId": "s1", "path": "notes.md", "line": 2, "limit": 1 } }),
                        );
                        loop {
                            let Some(Ok(line)) = reader.next() else {
                                return;
                            };
                            let m: Value = serde_json::from_str(&line).unwrap();
                            if m["id"] == json!(next_id) {
                                assert_eq!(m["result"]["content"], "two");
                                break;
                            }
                        }
                        next_id += 1;
                        send(
                            json!({ "jsonrpc": "2.0", "id": next_id, "method": "terminal/create", "params": {} }),
                        );
                        loop {
                            let Some(Ok(line)) = reader.next() else {
                                return;
                            };
                            let m: Value = serde_json::from_str(&line).unwrap();
                            if m["id"] == json!(next_id) {
                                assert_eq!(m["error"]["code"], -32601);
                                break;
                            }
                        }
                        send(update(
                            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "title": "Write notes.md", "kind": "edit", "status": "completed",
                            "content": [{ "type": "diff", "path": "notes.md", "oldText": "", "newText": "one\ntwo\nthree\n" }],
                            "locations": [{ "path": "notes.md" }] }),
                        ));
                        send(update(
                            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Done." } }),
                        ));
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                        );
                    }
                    _ => {}
                }
            }
        });
        let cancel = CancelToken::new();
        let (prompter, permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: dir,
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(Mode::default()),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools: None,
                resume: None,
                history: Vec::new(),
            },
            5,
            AcpFlavor::Generic,
        );
        (runtime, permissions)
    }

    fn drain_until_end(
        runtime: &AcpRuntime,
        permissions: &Receiver<PermissionEnvelope>,
        answer: PermissionAnswer,
    ) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut events = Vec::new();
        loop {
            events.extend(runtime.drain());
            if let Ok(envelope) = permissions.try_recv() {
                assert_eq!(envelope.request.tool, "edit");
                // The subject is derived from the call, as for the built-in
                // agent: the project-relative path, not the ACP title.
                assert_eq!(envelope.request.subject, "notes.md");
                envelope.reply.send(answer.clone()).unwrap();
            }
            if events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
                return events;
            }
            assert!(Instant::now() < deadline, "no AgentEnd: {events:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// An agent that advertises two models at `session/new` and accepts a
    /// `session/set_model`.
    fn models_agent(dir: PathBuf) -> AcpRuntime {
        models_agent_with(
            dir,
            json!({
                "models": {
                    "availableModels": [
                        { "modelId": "m-fast", "name": "Fast" },
                        { "modelId": "m-slow", "name": "Slow" }
                    ],
                    "currentModelId": "m-fast"
                }
            }),
        )
    }

    /// An agent that advertises the same two models as a `model` config
    /// option, one of them in a group, and switches through
    /// `session/set_config_option`.
    fn config_option_agent(dir: PathBuf) -> AcpRuntime {
        models_agent_with(
            dir,
            json!({
                "configOptions": [
                    { "id": "mode", "category": "mode", "type": "select",
                      "currentValue": "ask", "options": [{ "value": "ask", "name": "Ask" }] },
                    { "id": "model", "category": "model", "type": "select",
                      "currentValue": "m-fast",
                      "options": [
                          { "value": "m-fast", "name": "Fast" },
                          { "group": "older", "name": "Older",
                            "options": [{ "value": "m-slow", "name": "Slow" }] }
                      ] }
                ]
            }),
        )
    }

    /// An agent that records every message it gets and answers the
    /// handshake and `session/set_config_option`; it takes MCP servers over
    /// HTTP.
    fn recording_agent(
        dir: PathBuf,
        flavor: AcpFlavor,
        host_tools: Option<HostTools>,
        mode: ModeHandle,
    ) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
        recording_agent_with(dir, flavor, host_tools, mode, true)
    }

    /// [`recording_agent`], saying whether it takes MCP servers over HTTP.
    fn recording_agent_with(
        dir: PathBuf,
        flavor: AcpFlavor,
        host_tools: Option<HostTools>,
        mode: ModeHandle,
        http: bool,
    ) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut send = |value: Value| writeln!(out, "{value}").unwrap();
            let mut reader = BufReader::new(to_agent_rx).lines();
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                record.lock().unwrap().push(message.clone());
                let id = message["id"].clone();
                let result = match message["method"].as_str() {
                    Some("initialize") => json!({
                        "protocolVersion": 1,
                        "agentCapabilities": { "mcpCapabilities": { "http": http } },
                    }),
                    Some("session/new") => json!({ "sessionId": "s1" }),
                    Some(_) => json!({}),
                    None => continue,
                };
                send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
        });
        let cancel = CancelToken::new();
        let (prompter, _permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: dir,
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode,
                system_prompt: "termide's prompt".into(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools,
                resume: None,
                history: Vec::new(),
            },
            5,
            flavor,
        );
        (runtime, seen)
    }

    /// Wait until the agent has seen a message matching `wanted`.
    fn seen_where(seen: &Arc<Mutex<Vec<Value>>>, wanted: impl Fn(&Value) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let found: Vec<Value> = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|m| wanted(m))
                .cloned()
                .collect();
            if !found.is_empty() {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "never seen: {:?}",
                seen.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    struct Echo;

    impl termide_agent_core::Tool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Say it back"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object" })
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            _on_update: &mut dyn FnMut(termide_agent_core::ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            ToolResultMessage::text(call, "echoed").with_details(json!({ "said": "echoed" }))
        }
    }

    /// The names of the tools termide's MCP server at `url` lists.
    fn served_tools(url: &str, auth: &str) -> Vec<String> {
        let reply = post_mcp(
            url,
            auth,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        );
        reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// Send `body` to termide's MCP server at `url` and return its reply.
    fn post_mcp(url: &str, auth: &str, body: &Value) -> Value {
        use std::io::Read;
        let addr = url.trim_start_matches("http://").trim_end_matches("/mcp");
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        let body = body.to_string();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: {auth}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(body).unwrap()
    }

    #[test]
    fn a_call_of_termides_tool_ends_with_the_details_its_reply_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::ClaudeCode,
            Some(echo_host()),
            ModeHandle::new(Mode::default()),
        );
        let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
        let server = &new["params"]["mcpServers"][0];
        let url = server["url"].as_str().unwrap();
        let auth = server["headers"][0]["value"].as_str().unwrap();
        // As Claude Code goes about it: the call is announced, its arguments
        // follow, the server runs it, and the end carries the reply's text.
        let shared = &runtime.shared;
        shared.on_update(&json!({
            "sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "other",
            "status": "pending", "title": "mcp__termide__echo", "rawInput": {},
        }));
        shared.on_update(&json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "rawInput": { "text": "hi" },
        }));
        let reply = post_mcp(
            url,
            auth,
            &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                     "params": { "name": "echo", "arguments": { "text": "hi" } } }),
        );
        assert!(reply["result"].get("details").is_none());
        assert!(!reply.to_string().contains("said"), "not for the model");
        shared.on_update(&json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
            "content": [{ "type": "content", "content": { "type": "text", "text": "echoed" } }],
        }));
        let ended = runtime.drain().into_iter().find_map(|e| match e {
            AgentEvent::ToolExecutionEnd { result } => Some(result),
            _ => None,
        });
        let ended = ended.expect("the call ends");
        assert_eq!(ended.tool_name, "echo");
        assert_eq!(ended.details, Some(json!({ "said": "echoed" })));
    }

    #[test]
    fn claude_code_gets_termides_prompt_and_tools_and_none_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let mut tools = termide_agent_core::ToolRegistry::new();
        tools.insert(Arc::new(Echo));
        let host = HostTools {
            tools,
            hooks: Box::new(termide_agent_core::NoHooks),
            context: ToolContext::new(PathBuf::new()),
            skills: Vec::new(),
        };
        let (runtime, seen) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::ClaudeCode,
            Some(host),
            ModeHandle::new(Mode::default()),
        );
        let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
        let params = &new["params"];
        assert_eq!(params["_meta"]["systemPrompt"], "termide's prompt");
        let options = &params["_meta"]["claudeCode"]["options"];
        assert_eq!(options["tools"], json!([]));
        assert_eq!(options["settingSources"], json!([]));
        assert_eq!(options["allowedTools"], json!(["mcp__termide"]));
        let server = &params["mcpServers"][0];
        assert_eq!(server["type"], "http");
        assert_eq!(server["name"], "termide");
        assert!(server["url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:"));
        assert!(server["headers"][0]["value"]
            .as_str()
            .unwrap()
            .starts_with("Bearer "));
        assert!(runtime.follows_mode());
        assert!(runtime.runs_host_tools());
        // A change of the set reaches the running server.
        let url = server["url"].as_str().unwrap();
        let auth = server["headers"][0]["value"].as_str().unwrap();
        assert_eq!(served_tools(url, auth), ["echo"]);
        runtime
            .update_host_tools(termide_agent_core::ToolRegistry::new())
            .unwrap();
        assert!(served_tools(url, auth).is_empty());
        // Its calls of termide's tools show under their own names.
        let call = tool_call_of(&json!({
            "toolCallId": "t1", "title": "mcp__termide__bash", "kind": "other",
            "rawInput": { "command": "ls" }
        }));
        assert_eq!(
            (call.name.as_str(), &call.arguments),
            ("bash", &json!({ "command": "ls" }))
        );
    }

    /// Prompt `text` and wait for the turn to end; the text blocks the agent
    /// got for it.
    fn prompt_blocks(
        runtime: &AcpRuntime,
        seen: &Arc<Mutex<Vec<Value>>>,
        text: &str,
    ) -> Vec<String> {
        runtime.prompt(UserMessage::text(text)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !runtime
            .drain()
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd))
        {
            assert!(Instant::now() < deadline, "the turn never ended");
            std::thread::sleep(Duration::from_millis(5));
        }
        let sent = seen_where(seen, |m| {
            m["method"] == "session/prompt"
                && m["params"]["prompt"]
                    .as_array()
                    .and_then(|blocks| blocks.last())
                    .is_some_and(|block| block["text"] == text)
        })
        .remove(0);
        sent["params"]["prompt"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["text"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn claude_code_is_told_of_a_plan_mode_switch_before_the_next_turn() {
        let dir = tempfile::tempdir().unwrap();
        let mode = ModeHandle::new(Mode::Plan);
        let (runtime, seen) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::ClaudeCode,
            None,
            mode.clone(),
        );
        let plan = PlanPrompt::default();
        // Started in plan mode, its system prompt already says so.
        assert_eq!(prompt_blocks(&runtime, &seen, "one"), ["one"]);
        mode.set(Mode::Edit);
        let two = prompt_blocks(&runtime, &seen, "two");
        assert_eq!(two.len(), 2, "{two:?}");
        assert!(two[0].starts_with("<system-reminder>") && two[0].contains(&plan.leave));
        assert_eq!(prompt_blocks(&runtime, &seen, "three"), ["three"]);
        mode.set(Mode::Plan);
        let four = prompt_blocks(&runtime, &seen, "four");
        assert!(four[0].contains(&plan.instructions), "{four:?}");
        assert_eq!(four[1], "four");

        // Codex is put in its own plan mode instead: no note.
        let mode = ModeHandle::new(Mode::Plan);
        let (codex, seen) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::Codex,
            None,
            mode.clone(),
        );
        mode.set(Mode::Edit);
        assert_eq!(prompt_blocks(&codex, &seen, "go"), ["go"]);
    }

    #[test]
    fn claude_code_is_told_of_a_change_of_its_tools_before_the_next_turn() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::ClaudeCode,
            Some(echo_host()),
            ModeHandle::new(Mode::Configured),
        );
        assert_eq!(prompt_blocks(&runtime, &seen, "one"), ["one"]);
        // `recall` switched off, an MCP server's tool come.
        let mut tools = termide_agent_core::ToolRegistry::new();
        tools.insert(Arc::new(Echo));
        tools.insert(Arc::new(Named("skill")));
        tools.insert(Arc::new(Named("db__query")));
        runtime.update_host_tools(tools).unwrap();
        let two = prompt_blocks(&runtime, &seen, "two");
        assert_eq!(two.len(), 2, "{two:?}");
        assert!(
            two[0].contains("Now available: mcp__termide__db__query.")
                && two[0].contains("do not call them: mcp__termide__recall."),
            "{two:?}"
        );
        // Told once.
        assert_eq!(prompt_blocks(&runtime, &seen, "three"), ["three"]);
    }

    #[test]
    fn a_call_announced_without_arguments_shows_once_they_arrive() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, _) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::ClaudeCode,
            None,
            ModeHandle::new(Mode::default()),
        );
        let shared = &runtime.shared;
        shared.on_update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "t1",
            "title": "mcp__termide__bash", "kind": "other", "rawInput": {}, "status": "pending" }));
        let starts = |events: &[AgentEvent]| -> Vec<ToolCall> {
            events
                .iter()
                .filter_map(|e| match e {
                    AgentEvent::ToolExecutionStart { call } => Some(call.clone()),
                    _ => None,
                })
                .collect()
        };
        assert!(
            starts(&runtime.drain()).is_empty(),
            "not before its arguments"
        );
        shared.on_update(
            &json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "title": "mcp__termide__bash", "rawInput": { "command": "ls" } }),
        );
        shared.on_update(
            &json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "status": "completed", "content": [] }),
        );
        let events = runtime.drain();
        let started = starts(&events);
        assert_eq!(started.len(), 1, "{events:?}");
        assert_eq!(started[0].name, "bash");
        assert_eq!(started[0].arguments, json!({ "command": "ls" }));
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. })));
    }

    /// The agent's calls reach the session log as the built-in loop's do:
    /// calls made together in one message ahead of their results, and a call
    /// the turn left unfinished answered with an error, never left open.
    #[test]
    fn calls_are_logged_with_their_results_like_the_native_loops() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, _) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::ClaudeCode,
            None,
            ModeHandle::new(Mode::default()),
        );
        let shared = &runtime.shared;
        let call = |id: &str, command: &str| {
            json!({ "sessionUpdate": "tool_call", "toolCallId": id,
                "title": "mcp__termide__bash", "kind": "other",
                "rawInput": { "command": command }, "status": "pending" })
        };
        let done = |id: &str, text: &str| {
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed",
                "content": [{ "type": "content", "content": { "type": "text", "text": text } }] })
        };
        shared.on_update(&call("t1", "ls"));
        shared.on_update(&call("t2", "pwd"));
        shared.on_update(&done("t2", "/p"));
        shared.on_update(&done("t1", "a.rs"));
        shared.on_update(&call("t3", "sleep 9"));
        shared.close_unfinished_calls();

        let logged: Vec<String> = runtime
            .drain()
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::MessageEnd(Message::Assistant(a)) => Some(format!(
                    "calls:{}",
                    a.tool_calls()
                        .map(|c| format!(
                            "{}={}",
                            c.id,
                            c.arguments["command"].as_str().unwrap_or("")
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                )),
                AgentEvent::MessageEnd(Message::ToolResult(r)) => Some(format!(
                    "result:{}:{}:{}",
                    r.tool_call_id,
                    r.is_error,
                    r.plain_text()
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            logged,
            [
                "calls:t1=ls,t2=pwd",
                "result:t2:false:/p",
                "result:t1:false:a.rs",
                "calls:t3=sleep 9",
                "result:t3:true:The turn ended before this call finished.",
            ]
        );
    }

    #[test]
    fn the_reported_context_fill_and_size_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, _) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::Codex,
            None,
            ModeHandle::new(Mode::default()),
        );
        assert_eq!(runtime.context_usage(), None);
        runtime
            .shared
            .on_update(&json!({ "sessionUpdate": "usage_update", "used": 975, "size": 1_000_000 }));
        assert_eq!(runtime.context_usage(), Some((975, 1_000_000)));
    }

    #[test]
    fn a_turns_reported_usage_is_read() {
        let usage = usage_of(&json!({ "stopReason": "end_turn", "usage": {
            "inputTokens": 4, "outputTokens": 63, "cachedReadTokens": 919,
            "cachedWriteTokens": 1009, "totalTokens": 1995 } }));
        assert_eq!(
            (
                usage.input,
                usage.output,
                usage.cache_read,
                usage.cache_write
            ),
            (4, 63, 919, 1009)
        );
        assert_eq!(
            usage_of(&json!({ "stopReason": "end_turn" })),
            Usage::default()
        );
    }

    #[test]
    fn codex_is_put_in_the_modes_that_match_the_panels() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::Codex,
            None,
            ModeHandle::new(Mode::Configured),
        );
        let option = |config: &'static str, value: &'static str| {
            move |m: &Value| {
                m["method"] == "session/set_config_option"
                    && m["params"]["configId"] == config
                    && m["params"]["value"] == value
            }
        };
        // At start: everything asks, so termide decides.
        seen_where(&seen, option("mode", "read-only"));
        seen_where(&seen, option("collaboration_mode", "default"));
        let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
        assert!(
            new["params"].get("_meta").is_none(),
            "Codex keeps its prompt"
        );
        runtime.set_mode(Mode::Edit);
        seen_where(&seen, option("mode", "workspace-write"));
        runtime.set_mode(Mode::All);
        seen_where(&seen, option("mode", "agent-full-access"));
        runtime.set_mode(Mode::Plan);
        seen_where(&seen, option("collaboration_mode", "plan"));
    }

    /// A tool of termide's under `0`'s name, which does nothing.
    struct Named(&'static str);

    impl termide_agent_core::Tool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "Do it"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object" })
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            _on_update: &mut dyn FnMut(termide_agent_core::ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            ToolResultMessage::text(call, "done")
        }
    }

    /// termide's tools, as the panel offers them to every agent.
    fn echo_host() -> HostTools {
        let mut tools = termide_agent_core::ToolRegistry::new();
        tools.insert(Arc::new(Echo));
        tools.insert(Arc::new(Named("recall")));
        tools.insert(Arc::new(Named("skill")));
        HostTools {
            tools,
            hooks: Box::new(termide_agent_core::NoHooks),
            context: ToolContext::new(PathBuf::new()),
            skills: vec![termide_agent_core::SkillInfo {
                name: "review".into(),
                description: "Review a change".into(),
                argument_hint: "<path>".into(),
                path: PathBuf::new(),
            }],
        }
    }

    #[test]
    fn codex_and_gemini_keep_their_tools_and_are_served_the_mcp_servers() {
        for flavor in [AcpFlavor::Codex, AcpFlavor::GeminiCli] {
            let dir = tempfile::tempdir().unwrap();
            let (runtime, seen) = recording_agent(
                dir.path().to_path_buf(),
                flavor,
                Some(echo_host()),
                ModeHandle::new(Mode::Configured),
            );
            // It lists its tools once, so its session waits until termide's
            // MCP servers have answered.
            std::thread::sleep(Duration::from_millis(100));
            assert!(!seen
                .lock()
                .unwrap()
                .iter()
                .any(|m| m["method"] == "session/new"));
            runtime.host_tools_settled();
            let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
            let params = &new["params"];
            assert!(params.get("_meta").is_none(), "it keeps its own prompt");
            let server = &params["mcpServers"][0];
            assert_eq!(server["name"], "termide");
            assert!(!runtime.runs_host_tools());
            assert!(runtime.takes_mcp_tools());
            // Of termide's own tools, only those it has no counterpart of:
            // `echo` would stand beside its own.
            let url = server["url"].as_str().unwrap();
            let auth = server["headers"][0]["value"].as_str().unwrap();
            assert_eq!(served_tools(url, auth), ["recall", "skill"]);
            // The skills are listed where its own prompt does not.
            let listed = post_mcp(
                url,
                auth,
                &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            );
            let skill = listed["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "skill")
                .unwrap()
                .clone();
            assert!(
                skill["description"]
                    .as_str()
                    .unwrap()
                    .ends_with("- review <path>: Review a change"),
                "{skill}"
            );
            // An MCP server of termide's connected: its tools are served.
            let mut mcp = termide_agent_core::ToolRegistry::new();
            mcp.insert(Arc::new(Echo));
            runtime.update_host_tools(mcp).unwrap();
            assert_eq!(served_tools(url, auth), ["echo"]);
        }
    }

    #[test]
    fn a_codex_call_of_termides_tool_shows_as_it_and_is_not_asked_twice() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, _seen) = recording_agent_with(
            dir.path().to_path_buf(),
            AcpFlavor::Codex,
            None,
            ModeHandle::new(Mode::Configured),
            false,
        );
        let shared = &runtime.shared;
        // As Codex sends them: the call names the server and the tool, the
        // permission request names the call alone, the end carries the MCP
        // result.
        shared.on_update(&json!({
            "sessionUpdate": "tool_call", "toolCallId": "c1", "kind": "execute",
            "status": "in_progress", "title": "mcp.termide.vault__secret_word",
            "rawInput": { "server": "termide", "tool": "vault__secret_word", "arguments": { "a": 1 } },
            "_meta": { "is_mcp_tool_call": true },
        }));
        let answer = shared.ask_permission(&json!({
            "toolCall": { "toolCallId": "c1", "kind": "execute", "status": "pending" },
            "options": [
                { "kind": "allow_once", "optionId": "allow_once" },
                { "kind": "reject_once", "optionId": "cancel" },
            ],
            "_meta": { "is_mcp_tool_approval": true },
        }));
        assert_eq!(answer["outcome"]["optionId"], "allow_once");
        shared.on_update(&json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
            "rawOutput": { "result": { "content": [{ "type": "text", "text": "pineapple" }] } },
        }));
        let events = runtime.drain();
        let started = events.iter().find_map(|e| match e {
            AgentEvent::ToolExecutionStart { call } => Some(call.clone()),
            _ => None,
        });
        let started = started.expect("the call starts");
        assert_eq!(
            (started.name.as_str(), &started.arguments),
            ("vault__secret_word", &json!({ "a": 1 }))
        );
        let ended = events.iter().find_map(|e| match e {
            AgentEvent::ToolExecutionEnd { result } => Some(result.clone()),
            _ => None,
        });
        let ended = ended.expect("the call ends");
        assert_eq!(
            (ended.tool_name.as_str(), ended.plain_text().as_str()),
            ("vault__secret_word", "pineapple")
        );
        // Ended, it is a call like any other again.
        assert!(shared.host_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_agent_without_http_servers_is_served_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = recording_agent_with(
            dir.path().to_path_buf(),
            AcpFlavor::Codex,
            Some(echo_host()),
            ModeHandle::new(Mode::Configured),
            false,
        );
        let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
        assert_eq!(new["params"]["mcpServers"], json!([]));
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.takes_mcp_tools() {
            assert!(Instant::now() < deadline, "it still claims the tools");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(runtime
            .update_host_tools(termide_agent_core::ToolRegistry::new())
            .is_err());
    }

    #[test]
    fn gemini_cli_is_put_in_the_mode_that_matches_the_panels() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::GeminiCli,
            None,
            ModeHandle::new(Mode::Configured),
        );
        let mode = |value: &'static str| {
            move |m: &Value| m["method"] == "session/set_mode" && m["params"]["modeId"] == value
        };
        // At start: it asks, so termide decides.
        seen_where(&seen, mode("default"));
        let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
        assert!(
            new["params"].get("_meta").is_none(),
            "Gemini CLI keeps its prompt"
        );
        runtime.set_mode(Mode::Edit);
        seen_where(&seen, mode("autoEdit"));
        runtime.set_mode(Mode::All);
        seen_where(&seen, mode("yolo"));
        runtime.set_mode(Mode::Plan);
        seen_where(&seen, mode("plan"));
    }

    #[test]
    fn another_agent_is_left_to_its_own_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = recording_agent(
            dir.path().to_path_buf(),
            AcpFlavor::Generic,
            None,
            ModeHandle::new(Mode::default()),
        );
        let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
        assert_eq!(
            new["params"],
            json!({ "cwd": dir.path(), "mcpServers": [] })
        );
        assert!(!runtime.follows_mode());
    }

    fn models_agent_with(dir: PathBuf, advertised: Value) -> AcpRuntime {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut send = |value: Value| writeln!(out, "{value}").unwrap();
            let mut reader = BufReader::new(to_agent_rx).lines();
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                let id = message["id"].clone();
                match message["method"].as_str() {
                    Some("initialize") => send(
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": 1, "agentCapabilities": {} } }),
                    ),
                    Some("session/new") => {
                        let mut result = advertised.clone();
                        result["sessionId"] = json!("s1");
                        send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                    }
                    Some("session/set_model") => {
                        assert_eq!(message["params"]["modelId"], "m-slow");
                        send(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                    }
                    Some("session/set_config_option") => {
                        assert_eq!(message["params"]["configId"], "model");
                        assert_eq!(message["params"]["value"], "m-slow");
                        send(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                    }
                    _ => {}
                }
            }
        });
        let cancel = CancelToken::new();
        let (prompter, _permissions) = permission_channel(cancel.clone());
        AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: dir,
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(Mode::default()),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools: None,
                resume: None,
                history: Vec::new(),
            },
            5,
            AcpFlavor::Generic,
        )
    }

    #[test]
    fn models_are_read_from_the_session_and_switched_over_acp() {
        for agent in [models_agent, config_option_agent] {
            let dir = tempfile::tempdir().unwrap();
            let runtime = agent(dir.path().to_path_buf());
            switches_between_fast_and_slow(&runtime);
        }
    }

    /// The config options as an agent states them, effort at `effort`.
    fn stated_options(effort: &str) -> Value {
        json!([
            { "id": "model", "category": "model", "type": "select", "currentValue": "m-fast",
              "options": [{ "value": "m-fast", "name": "Fast" }] },
            { "id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
              "currentValue": effort,
              "options": [{ "value": "low", "name": "Low" }, { "value": "high", "name": "High" }] },
            { "id": "brave", "name": "Brave", "type": "boolean", "currentValue": true }
        ])
    }

    #[test]
    fn options_are_listed_and_set_while_a_turn_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut send = |value: Value| writeln!(out, "{value}").unwrap();
            let mut reader = BufReader::new(to_agent_rx).lines();
            let mut prompt_id = None;
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                let id = message["id"].clone();
                let result = match message["method"].as_str() {
                    Some("initialize") => json!({ "protocolVersion": 1, "agentCapabilities": {} }),
                    Some("session/new") => {
                        send(
                            json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                            "sessionId": "s1", "update": {
                                "sessionUpdate": "available_commands_update",
                                "availableCommands": [
                                    { "name": "compact", "description": "Compact the context" },
                                    { "name": "review", "description": "Review", "input": { "hint": "<path>" } }
                                ] } } }),
                        );
                        json!({ "sessionId": "s1", "configOptions": stated_options("low") })
                    }
                    // The turn stays open until the option is set.
                    Some("session/prompt") => {
                        prompt_id = Some(id);
                        continue;
                    }
                    Some("session/set_config_option") => {
                        assert_eq!(message["params"]["configId"], "effort");
                        let value = message["params"]["value"].as_str().unwrap();
                        send(json!({ "jsonrpc": "2.0", "id": id,
                            "result": { "configOptions": stated_options(value) } }));
                        if let Some(prompt) = prompt_id.take() {
                            send(json!({ "jsonrpc": "2.0", "id": prompt,
                                "result": { "stopReason": "end_turn" } }));
                        }
                        continue;
                    }
                    _ => continue,
                };
                send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
        });
        let cancel = CancelToken::new();
        let (prompter, _permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: dir.path().to_path_buf(),
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(Mode::default()),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools: None,
                resume: None,
                history: Vec::new(),
            },
            5,
            AcpFlavor::Generic,
        );
        runtime.prompt(UserMessage::text("go")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.config_options().is_empty() {
            assert!(Instant::now() < deadline, "options never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
        // The model is listed as models, a boolean is not understood: the
        // effort alone remains.
        let options = runtime.config_options();
        assert_eq!(options.len(), 1, "{options:?}");
        assert!(options[0].is("thought_level"));
        assert_eq!(options[0].current_name(), "Low");
        assert_eq!(runtime.current_model(), Some("m-fast".to_string()));
        // Its own commands are listed.
        let commands = runtime.agent_commands();
        assert_eq!(
            commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["compact", "review"]
        );
        assert_eq!(commands[1].hint, "<path>");

        assert!(runtime.is_busy());
        runtime
            .set_option("effort".to_string(), "high".to_string())
            .unwrap();
        assert_eq!(runtime.config_options()[0].current, "high");
    }

    /// An agent that steers: its turn for "first" streams a word, then waits
    /// for a `_session/steering` request, answers it `outcome`, streams
    /// another word and ends. Every prompt's text is recorded.
    fn steering_agent(dir: PathBuf, outcome: &'static str) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut send = |value: Value| writeln!(out, "{value}").unwrap();
            let update = |u: Value| json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s1", "update": u } });
            let chunk = |text: &str| {
                update(
                    json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } }),
                )
            };
            let mut reader = BufReader::new(to_agent_rx).lines();
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                record.lock().unwrap().push(message.clone());
                let id = message["id"].clone();
                match message["method"].as_str() {
                    Some("initialize") => send(json!({ "jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": 1, "agentCapabilities": {},
                        "_meta": { "steering": { "supported": true } } } })),
                    Some("session/new") => {
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "sessionId": "s1" } }),
                        );
                    }
                    Some("session/prompt") => {
                        let text = message["params"]["prompt"][0]["text"]
                            .as_str()
                            .unwrap_or("");
                        if text != "first" {
                            send(chunk("later"));
                            send(
                                json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                            );
                            continue;
                        }
                        send(chunk("working"));
                        let steer = loop {
                            let Some(Ok(line)) = reader.next() else {
                                return;
                            };
                            let m: Value = serde_json::from_str(&line).unwrap();
                            record.lock().unwrap().push(m.clone());
                            if m["method"] == "_session/steering" {
                                break m;
                            }
                        };
                        if outcome == "injected" {
                            send(
                                json!({ "jsonrpc": "2.0", "id": steer["id"], "result": { "outcome": "injected" } }),
                            );
                            send(chunk("noted"));
                            send(
                                json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                            );
                        } else {
                            // The turn is over before the message could join it.
                            send(
                                json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                            );
                            send(
                                json!({ "jsonrpc": "2.0", "id": steer["id"], "result": { "outcome": outcome, "reason": "noRunningTurn" } }),
                            );
                        }
                    }
                    _ => {}
                }
            }
        });
        let cancel = CancelToken::new();
        let (prompter, _permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: dir,
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(Mode::default()),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools: None,
                resume: None,
                history: Vec::new(),
            },
            5,
            AcpFlavor::Generic,
        );
        (runtime, seen)
    }

    /// Run "first", steer "more" once the turn streams, and collect the
    /// events until the run ends.
    fn steer_into_first(runtime: &AcpRuntime) -> Vec<AgentEvent> {
        runtime.prompt(UserMessage::text("first")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        let mut steered = false;
        loop {
            events.extend(runtime.drain());
            let streaming = events
                .iter()
                .any(|e| matches!(e, AgentEvent::MessageUpdate(StreamEvent::TextDelta(t)) if t == "working"));
            if streaming && !steered {
                runtime.steer(UserMessage::text("more"));
                steered = true;
            }
            if steered
                && events
                    .iter()
                    .filter(|e| matches!(e, AgentEvent::AgentEnd))
                    .count()
                    >= 1
            {
                return events;
            }
            assert!(Instant::now() < deadline, "no end: {events:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The messages logged, in order: `user:` or `assistant:` and the text.
    fn logged(events: &[AgentEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::MessageEnd(Message::User(u)) => {
                    Some(format!("user:{}", u.plain_text()))
                }
                AgentEvent::MessageEnd(Message::Assistant(a)) => {
                    Some(format!("assistant:{}", a.plain_text()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_message_typed_during_a_turn_joins_it_when_the_agent_steers() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = steering_agent(dir.path().to_path_buf(), "injected");
        let events = steer_into_first(&runtime);
        assert_eq!(
            logged(&events),
            [
                "user:first",
                "assistant:working",
                "user:more",
                "assistant:noted"
            ]
        );
        let steering = seen_where(&seen, |m| m["method"] == "_session/steering");
        assert_eq!(steering[0]["params"]["prompt"][0]["text"], "more");
        assert_eq!(
            steering[0]["params"]["_meta"]["steering"]["idleBehavior"],
            "promptRequired"
        );
        // It went into the turn, not as a turn of its own.
        let prompts = seen_where(&seen, |m| m["method"] == "session/prompt");
        assert_eq!(prompts.len(), 1);
        assert_eq!(runtime.queue_lens(), (0, 0));
    }

    #[test]
    fn a_message_the_turn_could_not_take_goes_as_the_next_turn() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = steering_agent(dir.path().to_path_buf(), "promptRequired");
        let mut events = steer_into_first(&runtime);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !logged(&events).contains(&"assistant:later".to_string()) {
            assert!(Instant::now() < deadline, "no second turn: {events:?}");
            std::thread::sleep(Duration::from_millis(5));
            events.extend(runtime.drain());
        }
        assert_eq!(
            logged(&events),
            [
                "user:first",
                "assistant:working",
                "user:more",
                "assistant:later"
            ]
        );
        let prompts = seen_where(&seen, |m| {
            m["method"] == "session/prompt" && m["params"]["prompt"][0]["text"] == "more"
        });
        assert_eq!(prompts.len(), 1);
    }

    /// An agent that forks and closes sessions when `forks` says so, and
    /// answers every prompt with `answer`, streamed in the prompt's session.
    fn side_agent(
        dir: PathBuf,
        forks: bool,
        answer: &'static str,
    ) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut send = |value: Value| writeln!(out, "{value}").unwrap();
            let mut reader = BufReader::new(to_agent_rx).lines();
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                record.lock().unwrap().push(message.clone());
                let id = message["id"].clone();
                let result = match message["method"].as_str() {
                    Some("initialize") => {
                        let sessions = if forks {
                            json!({ "fork": {}, "close": {} })
                        } else {
                            json!({})
                        };
                        json!({ "protocolVersion": 1,
                            "agentCapabilities": { "sessionCapabilities": sessions } })
                    }
                    Some("session/new") => json!({ "sessionId": "s1" }),
                    Some("session/fork") => json!({ "sessionId": "f1" }),
                    Some("session/prompt") => {
                        let session = message["params"]["sessionId"].clone();
                        send(
                            json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                            "sessionId": session, "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": answer } } } }),
                        );
                        json!({ "stopReason": "end_turn" })
                    }
                    Some(_) => json!({}),
                    None => continue,
                };
                send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
        });
        let cancel = CancelToken::new();
        let (prompter, _permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: dir,
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(Mode::default()),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools: None,
                resume: None,
                history: Vec::new(),
            },
            5,
            AcpFlavor::Generic,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.shared.session_id().is_err() {
            assert!(Instant::now() < deadline, "the session never opened");
            std::thread::sleep(Duration::from_millis(5));
        }
        (runtime, seen)
    }

    /// The events until one `wanted` arrives.
    fn events_until(runtime: &AcpRuntime, wanted: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while !events.iter().any(&wanted) {
            assert!(Instant::now() < deadline, "never came: {events:?}");
            std::thread::sleep(Duration::from_millis(5));
            events.extend(runtime.drain());
        }
        events
    }

    #[test]
    fn the_goal_judge_asks_a_fork_of_the_session_and_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = side_agent(dir.path().to_path_buf(), true, "DONE\nthe tests pass");
        runtime.judge("make the tests pass".into()).unwrap();
        let events = events_until(&runtime, |e| matches!(e, AgentEvent::GoalJudged { .. }));
        assert!(events.contains(&AgentEvent::GoalJudged {
            done: true,
            reason: "the tests pass".into()
        }));
        // Nothing of it reached the transcript.
        assert!(!events
            .iter()
            .any(|e| matches!(e, AgentEvent::MessageEnd(_))));
        let prompt = seen_where(&seen, |m| m["method"] == "session/prompt").remove(0);
        assert_eq!(prompt["params"]["sessionId"], "f1");
        let text = prompt["params"]["prompt"][0]["text"].as_str().unwrap();
        assert!(text.contains("make the tests pass"), "{text}");
        let close = seen_where(&seen, |m| m["method"] == "session/close").remove(0);
        assert_eq!(close["params"]["sessionId"], "f1");
        assert!(!runtime.is_busy());
    }

    #[test]
    fn a_handoff_without_a_fork_is_a_turn_kept_from_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = side_agent(dir.path().to_path_buf(), false, "The brief.");
        runtime.handoff().unwrap();
        let events = events_until(&runtime, |e| matches!(e, AgentEvent::Handoff { .. }));
        assert!(events.contains(&AgentEvent::Handoff {
            brief: Ok("The brief.".into())
        }));
        assert!(!events
            .iter()
            .any(|e| matches!(e, AgentEvent::MessageEnd(_))));
        let prompt = seen_where(&seen, |m| m["method"] == "session/prompt").remove(0);
        assert_eq!(prompt["params"]["sessionId"], "s1");
        assert!(prompt["params"]["prompt"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with(&HandoffPrompt::default().request));
        assert!(!seen
            .lock()
            .unwrap()
            .iter()
            .any(|m| m["method"] == "session/fork"));
        // The session is free again for the next request.
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.is_busy() {
            assert!(Instant::now() < deadline, "still busy");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A reviewer that answers `verdict` and keeps what it was shown.
    struct Reviewer {
        verdict: &'static str,
        shown: Arc<Mutex<Vec<String>>>,
    }

    impl Provider for Reviewer {
        fn name(&self) -> &str {
            "reviewer"
        }
        fn stream(
            &self,
            request: &termide_agent_core::Request<'_>,
            _on_event: &mut dyn FnMut(StreamEvent),
            _cancel: &CancelToken,
        ) -> AssistantMessage {
            for message in request.messages {
                if let Message::User(user) = message {
                    self.shown.lock().unwrap().push(user.plain_text());
                }
            }
            AssistantMessage {
                content: vec![AssistantContent::Text {
                    text: self.verdict.into(),
                }],
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                provider: "reviewer".into(),
                model: request.model.id.clone(),
                error_message: None,
                timestamp: 0,
            }
        }
    }

    /// An agent whose turn asks to run `cargo build` and reports the option
    /// it was answered with.
    fn asking_agent(
        dir: PathBuf,
        mode: Mode,
        reviewer: ReviewerSetup,
    ) -> (AcpRuntime, Receiver<PermissionEnvelope>) {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut send = |value: Value| writeln!(out, "{value}").unwrap();
            let mut reader = BufReader::new(to_agent_rx).lines();
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                let id = message["id"].clone();
                match message["method"].as_str() {
                    Some("initialize") => send(json!({ "jsonrpc": "2.0", "id": id,
                        "result": { "protocolVersion": 1, "agentCapabilities": {} } })),
                    Some("session/new") => send(json!({ "jsonrpc": "2.0", "id": id,
                        "result": { "sessionId": "s1" } })),
                    Some("session/prompt") => {
                        send(
                            json!({ "jsonrpc": "2.0", "id": 900, "method": "session/request_permission",
                            "params": { "sessionId": "s1",
                                "toolCall": { "toolCallId": "t1", "title": "cargo build", "kind": "execute",
                                    "rawInput": { "command": "cargo build" } },
                                "options": [
                                    { "optionId": "yes", "name": "Allow", "kind": "allow_once" },
                                    { "optionId": "no", "name": "Reject", "kind": "reject_once" } ] } }),
                        );
                        let answer: Value = loop {
                            let Some(Ok(line)) = reader.next() else {
                                return;
                            };
                            let m: Value = serde_json::from_str(&line).unwrap();
                            if m["id"] == 900 {
                                break m;
                            }
                        };
                        let chosen = answer["result"]["outcome"]["optionId"]
                            .as_str()
                            .unwrap_or("cancelled")
                            .to_string();
                        send(
                            json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                            "sessionId": "s1", "update": { "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": chosen } } } }),
                        );
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                        );
                    }
                    Some(_) if !id.is_null() => {
                        send(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                    }
                    _ => {}
                }
            }
        });
        let cancel = CancelToken::new();
        let (prompter, permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: dir,
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(mode),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer,
                host_tools: None,
                resume: None,
                history: vec![Message::User(UserMessage::text("set the project up"))],
            },
            5,
            AcpFlavor::Codex,
        );
        (runtime, permissions)
    }

    #[test]
    fn in_auto_mode_the_reviewer_answers_an_external_agents_request() {
        for (verdict, chosen) in [
            ("ALLOW it builds the project", "yes"),
            ("BLOCK not asked for", "no"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let shown = Arc::new(Mutex::new(Vec::new()));
            let reviewer = ReviewerSetup {
                model: termide_agent_core::ModelChoice::own(
                    Arc::new(Reviewer {
                        verdict,
                        shown: Arc::clone(&shown),
                    }),
                    ModelSpec {
                        provider: "reviewer".into(),
                        id: "small".into(),
                        context_window: 0,
                        max_tokens: None,
                        thinking: ThinkingLevel::Off,
                    },
                ),
                ..ReviewerSetup::default()
            };
            let (runtime, permissions) =
                asking_agent(dir.path().to_path_buf(), Mode::Auto, reviewer);
            runtime.prompt(UserMessage::text("build it")).unwrap();
            let events = events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
            // Nobody was asked; the reviewer decided.
            assert!(permissions.try_recv().is_err());
            assert!(
                logged(&events).contains(&format!("assistant:{chosen}")),
                "{events:?}"
            );
            // It judged against what the user asked, earlier and now.
            let shown = shown.lock().unwrap().join("\n");
            assert!(
                shown.contains("set the project up") && shown.contains("build it"),
                "{shown}"
            );
        }
    }

    #[test]
    fn without_a_model_to_review_with_the_user_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        // Started from streams: no subscription is "the session's model".
        let (runtime, permissions) = asking_agent(
            dir.path().to_path_buf(),
            Mode::Auto,
            ReviewerSetup::default(),
        );
        runtime.prompt(UserMessage::text("build it")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let envelope = loop {
            if let Ok(envelope) = permissions.try_recv() {
                break envelope;
            }
            assert!(Instant::now() < deadline, "nobody was asked");
            std::thread::sleep(Duration::from_millis(5));
        };
        envelope.reply.send(PermissionAnswer::AllowOnce).unwrap();
        let events = events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
        assert!(logged(&events).contains(&"assistant:yes".to_string()));
    }

    fn switches_between_fast_and_slow(runtime: &AcpRuntime) {
        // The handshake runs on a thread; wait for the advertised models.
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.available_models().is_empty() {
            assert!(Instant::now() < deadline, "models never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            runtime.available_models(),
            vec![
                BackendModel {
                    id: "m-fast".into(),
                    name: "Fast".into()
                },
                BackendModel {
                    id: "m-slow".into(),
                    name: "Slow".into()
                },
            ]
        );
        assert_eq!(runtime.current_model(), Some("m-fast".to_string()));

        runtime.select_model("m-slow".to_string()).unwrap();
        assert_eq!(runtime.current_model(), Some("m-slow".to_string()));
    }

    #[test]
    fn a_turn_streams_asks_edits_and_ends_like_the_native_loop() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, permissions) = fake_agent(dir.path().to_path_buf());
        // Sent before the handshake finished: queued, then delivered.
        runtime.prompt(UserMessage::text("first")).unwrap();
        assert!(runtime.is_busy());
        assert_eq!(
            runtime.prompt(UserMessage::text("x")),
            Err(PromptError::Busy)
        );
        let events = drain_until_end(&runtime, &permissions, PermissionAnswer::AllowSession);
        assert!(!runtime.is_busy());
        // The reasoning is kept with the text it led to, for the log.
        let first = events.iter().find_map(|e| match e {
            AgentEvent::MessageEnd(Message::Assistant(a)) => Some(a),
            _ => None,
        });
        assert_eq!(
            first.map(AssistantMessage::thinking_text).as_deref(),
            Some("hmm")
        );

        let mut kinds: Vec<String> = events
            .iter()
            .map(|e| match e {
                AgentEvent::AgentStart => "start".into(),
                AgentEvent::TurnStart => "turn".into(),
                AgentEvent::MessageStart { .. } => "msg-start".into(),
                AgentEvent::MessageUpdate(StreamEvent::TextDelta(t)) => format!("text:{t}"),
                AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(_)) => "think".into(),
                AgentEvent::MessageUpdate(_) => "update".into(),
                AgentEvent::MessageEnd(Message::User(u)) => format!("user:{}", u.plain_text()),
                AgentEvent::MessageEnd(Message::Assistant(a)) => {
                    let calls: Vec<&str> = a.tool_calls().map(|c| c.name.as_str()).collect();
                    if calls.is_empty() {
                        format!("assistant:{}:{:?}", a.plain_text(), a.stop_reason)
                    } else {
                        format!("calls:{}", calls.join(","))
                    }
                }
                AgentEvent::MessageEnd(Message::ToolResult(r)) => format!("result:{}", r.tool_name),
                AgentEvent::ToolExecutionStart { call } => {
                    format!("tool-start:{}:{}", call.name, call.arguments["title"])
                }
                AgentEvent::ToolExecutionUpdate { .. } => "tool-update".into(),
                AgentEvent::ToolExecutionEnd { result } => format!(
                    "tool-end:{}:{}",
                    result.tool_name,
                    result
                        .details
                        .as_ref()
                        .map(|d| d["path"].as_str().unwrap_or("").ends_with("notes.md"))
                        .unwrap_or(false)
                ),
                AgentEvent::TurnEnd => "turn-end".into(),
                AgentEvent::QueueUpdate { .. } => "queue".into(),
                AgentEvent::AgentEnd => "end".into(),
                AgentEvent::ExternalSession { session_id, .. } => format!("session:{session_id}"),
                _ => "other".into(),
            })
            .collect();
        // The handshake runs on its own thread, so where the session it
        // opened is reported among the turn's first events is not fixed.
        let opened = kinds.iter().position(|k| k == "session:s1");
        assert!(opened.is_some(), "the new session is reported: {kinds:?}");
        kinds.remove(opened.unwrap());
        assert_eq!(
            kinds,
            [
                "start",
                "turn",
                "user:first",
                "queue",
                "msg-start",
                "think",
                "text:Editing ",
                "text:now.",
                "assistant:Editing now.:ToolUse",
                "tool-start:edit:\"Write notes.md\"",
                "tool-start:write:null",
                "tool-end:write:true",
                "tool-end:edit:true",
                // The agent's call reaches the log as the built-in loop's
                // would; the client-side write it made is not a call of its.
                "calls:edit",
                "result:edit",
                "msg-start",
                "text:Done.",
                "assistant:Done.:Stop",
                "turn-end",
                "queue",
                "end"
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes.md")).unwrap(),
            "one\ntwo\nthree\n"
        );
        assert_eq!(
            runtime.update(Box::new(|_| {})),
            Err(PromptError::Unsupported)
        );

        // The second prompt is cancelled through session/cancel.
        runtime.prompt(UserMessage::text("second")).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        runtime.steer(UserMessage::text("later"));
        assert_eq!(runtime.queue_lens(), (1, 0));
        runtime.abort();
        let events = drain_until_end(&runtime, &permissions, PermissionAnswer::Deny);
        assert!(events.iter().any(|e| matches!(e, AgentEvent::MessageEnd(Message::Assistant(a)) if a.stop_reason == StopReason::Aborted)));
        // An abort drops what was steered.
        assert_eq!(runtime.queue_lens(), (0, 0));
    }

    /// A message typed while a turn runs leaves the queue strip as soon as
    /// the next turn takes it, not when that turn ends.
    #[test]
    fn a_message_taken_by_the_next_turn_leaves_the_queue_at_once() {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (prompted_tx, prompted_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut prompts = 0;
            for line in BufReader::new(to_agent_rx).lines() {
                let Ok(line) = line else { break };
                let message: Value = serde_json::from_str(&line).unwrap();
                let result = match message["method"].as_str() {
                    Some("initialize") => json!({ "protocolVersion": 1 }),
                    Some("session/new") => json!({ "sessionId": "s1" }),
                    Some("session/prompt") => {
                        prompts += 1;
                        if prompts == 1 {
                            // The first turn runs until the test lets it end.
                            prompted_tx.send(()).unwrap();
                            release_rx.recv().unwrap();
                        }
                        json!({ "stopReason": "end_turn" })
                    }
                    Some(_) => json!({}),
                    None => continue,
                };
                let reply = json!({ "jsonrpc": "2.0", "id": message["id"], "result": result });
                writeln!(out, "{reply}").unwrap();
            }
        });
        let cancel = CancelToken::new();
        let (prompter, permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: PathBuf::from("/tmp"),
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(Mode::default()),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools: None,
                resume: None,
                history: Vec::new(),
            },
            1,
            AcpFlavor::Generic,
        );
        runtime.prompt(UserMessage::text("first")).unwrap();
        prompted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.steer(UserMessage::text("later"));
        release_tx.send(()).unwrap();
        let events = drain_until_end(&runtime, &permissions, PermissionAnswer::Deny);

        let taken = events
            .iter()
            .position(|e| matches!(e, AgentEvent::MessageEnd(Message::User(u)) if u.plain_text() == "later"))
            .expect("the queued message runs as the next turn");
        let after: Vec<&AgentEvent> = events[taken..]
            .iter()
            .filter(|e| matches!(e, AgentEvent::QueueUpdate { .. } | AgentEvent::TurnEnd))
            .collect();
        assert!(
            matches!(
                after.first(),
                Some(AgentEvent::QueueUpdate {
                    steering: 0,
                    follow_up: 0
                })
            ),
            "the queue empties before the turn ends: {after:?}"
        );
    }

    #[test]
    fn a_failed_handshake_is_reported_on_the_first_prompt() {
        let (_keep, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        drop(from_agent_tx);
        let cancel = CancelToken::new();
        let (prompter, _permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "dead",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: PathBuf::from("/tmp"),
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(Mode::default()),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools: None,
                resume: None,
                history: Vec::new(),
            },
            1,
            AcpFlavor::Generic,
        );
        // The handshake fails as soon as the agent's closed stdout is read.
        // Racing that, the first prompt is either accepted (and the failure
        // arrives as an error event) or already refused because the runtime
        // has stopped; both report the dead handshake on the first prompt.
        if runtime.prompt(UserMessage::text("hello")).is_ok() {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut events = Vec::new();
            while !events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
                events.extend(runtime.drain());
                assert!(Instant::now() < deadline, "{events:?}");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(events.iter().any(|e| matches!(e, AgentEvent::MessageEnd(Message::Assistant(a)) if a.stop_reason == StopReason::Error && a.error_message.is_some())));
        }
        assert_eq!(
            runtime.prompt(UserMessage::text("again")),
            Err(PromptError::Stopped)
        );
    }

    #[test]
    fn an_execute_request_maps_to_a_bash_call() {
        let call = permission_call(&json!({
            "toolCallId": "t1",
            "title": "Run cat",
            "kind": "execute",
            "rawInput": { "command": "cat notes.txt" }
        }));
        assert_eq!(call.name, "bash");
        assert_eq!(call.arguments["command"], "cat notes.txt");

        // With no raw command, the title stands in so a read-only check works.
        let from_title = permission_call(&json!({
            "toolCallId": "t2", "title": "ls -la", "kind": "execute"
        }));
        assert_eq!(from_title.name, "bash");
        assert_eq!(from_title.arguments["command"], "ls -la");
    }

    #[test]
    fn a_read_only_execute_request_is_allowed_without_asking() {
        // Exactly what `ask_permission` does: map the ACP request, then let
        // the same hooks the built-in agent uses decide it.
        let call = permission_call(&json!({
            "toolCallId": "t1",
            "title": "Run cat",
            "kind": "execute",
            "rawInput": { "command": "cat notes.txt" }
        }));
        let cancel = CancelToken::new();
        let (prompter, rx) = permission_channel(cancel);
        let mut hooks = PermissionHooks::new(PermissionRules::default(), Box::new(prompter));
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        assert_eq!(hooks.before_tool_call(&call, &ctx), ToolDecision::Allow);
        assert!(
            rx.try_recv().is_err(),
            "a read-only command must not reach the user"
        );
    }

    /// An agent whose `initialize` advertises `capabilities` and which knows
    /// session `old`: `session/resume` and `session/load` take it up (unless
    /// `forgotten`), the load replaying an exchange first; `session/new`
    /// opens `fresh`; a prompt is answered "ok".
    fn session_agent(
        dir: PathBuf,
        capabilities: Value,
        forgotten: bool,
        resume: Option<ExternalSessionRef>,
        history: Vec<Message>,
    ) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut out = from_agent_tx;
            let mut send = |value: Value| writeln!(out, "{value}").unwrap();
            let mut reader = BufReader::new(to_agent_rx).lines();
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                record.lock().unwrap().push(message.clone());
                let id = message["id"].clone();
                let session = message["params"]["sessionId"].clone();
                let update = |u: Value| json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": u } });
                let result = match message["method"].as_str() {
                    Some("initialize") => {
                        json!({ "protocolVersion": 1, "agentCapabilities": capabilities })
                    }
                    Some("session/resume" | "session/load") if forgotten => {
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32002, "message": "no such session" } }),
                        );
                        continue;
                    }
                    Some("session/load") => {
                        send(update(
                            json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "old request" } }),
                        ));
                        send(update(
                            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "old answer" } }),
                        ));
                        send(update(
                            json!({ "sessionUpdate": "tool_call", "toolCallId": "old-call", "title": "Read", "kind": "read", "rawInput": {} }),
                        ));
                        json!({})
                    }
                    Some("session/resume") => json!({}),
                    Some("session/new") => json!({ "sessionId": "fresh" }),
                    Some("session/prompt") => {
                        send(update(
                            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "ok" } }),
                        ));
                        json!({ "stopReason": "end_turn" })
                    }
                    Some(_) => json!({}),
                    None => continue,
                };
                send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
        });
        let cancel = CancelToken::new();
        let (prompter, _permissions) = permission_channel(cancel.clone());
        let runtime = AcpRuntime::from_streams(
            "fake",
            from_agent_rx,
            to_agent_tx,
            BackendSetup {
                cwd: dir,
                prompter,
                cancel,
                rules: PermissionRules::default(),
                persist: None,
                mode: ModeHandle::new(Mode::default()),
                system_prompt: String::new(),
                plan: PlanPrompt::default(),
                goal: GoalPrompt::default(),
                handoff: HandoffPrompt::default(),
                reviewer: ReviewerSetup::default(),
                host_tools: None,
                resume,
                history,
            },
            5,
            AcpFlavor::Generic,
        );
        (runtime, seen)
    }

    fn old_session(agent: &str) -> Option<ExternalSessionRef> {
        Some(ExternalSessionRef {
            agent: agent.into(),
            session_id: "old".into(),
        })
    }

    fn earlier() -> Vec<Message> {
        vec![
            Message::User(UserMessage::text("earlier request")),
            Message::Assistant(AssistantMessage {
                content: vec![AssistantContent::Text {
                    text: "earlier answer".into(),
                }],
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                provider: ACP_PROVIDER.into(),
                model: "fake".into(),
                error_message: None,
                timestamp: 0,
            }),
        ]
    }

    /// Prompt `text`, wait for the turn to end and return every event since
    /// the runtime started, with the prompt the agent got.
    fn run_prompt(
        runtime: &AcpRuntime,
        seen: &Arc<Mutex<Vec<Value>>>,
        text: &str,
    ) -> (Vec<AgentEvent>, Vec<Value>) {
        runtime.prompt(UserMessage::text(text)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while !events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
            assert!(
                Instant::now() < deadline,
                "the turn never ended: {events:?}"
            );
            events.extend(runtime.drain());
            std::thread::sleep(Duration::from_millis(5));
        }
        let sent = seen_where(seen, |m| m["method"] == "session/prompt").remove(0);
        (events, sent["params"]["prompt"].as_array().unwrap().clone())
    }

    fn methods(seen: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .filter_map(|m| m["method"].as_str().map(str::to_string))
            .collect()
    }

    fn opened(events: &[AgentEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ExternalSession { agent, session_id } => {
                    Some(format!("{agent}:{session_id}"))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_agent_that_resumes_takes_its_own_session_up_again() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = session_agent(
            dir.path().to_path_buf(),
            json!({ "loadSession": true, "sessionCapabilities": { "resume": {} } }),
            false,
            old_session("fake"),
            earlier(),
        );
        let (events, prompt) = run_prompt(&runtime, &seen, "next");
        assert_eq!(
            methods(&seen),
            ["initialize", "session/resume", "session/prompt"]
        );
        let resumed = seen_where(&seen, |m| m["method"] == "session/resume").remove(0);
        assert_eq!(resumed["params"]["sessionId"], "old");
        assert_eq!(resumed["params"]["cwd"], json!(dir.path()));
        // The prompt goes to the resumed session, alone: the agent knows the rest.
        let sent = seen_where(&seen, |m| m["method"] == "session/prompt").remove(0);
        assert_eq!(sent["params"]["sessionId"], "old");
        assert_eq!(prompt, [json!({ "type": "text", "text": "next" })]);
        assert!(opened(&events).is_empty(), "nothing new to record");
    }

    #[test]
    fn a_loaded_session_replays_nothing_into_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = session_agent(
            dir.path().to_path_buf(),
            json!({ "loadSession": true }),
            false,
            old_session("fake"),
            earlier(),
        );
        let (events, prompt) = run_prompt(&runtime, &seen, "next");
        assert_eq!(
            methods(&seen),
            ["initialize", "session/load", "session/prompt"]
        );
        assert_eq!(prompt.len(), 1);
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::MessageUpdate(StreamEvent::TextDelta(delta)) => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            text, "ok",
            "the replay must not reach the panel: {events:?}"
        );
        assert!(!events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. })));
    }

    #[test]
    fn a_new_session_is_recorded_and_told_a_recap_once() {
        let dir = tempfile::tempdir().unwrap();
        // No way to resume: the agent advertises none.
        let (runtime, seen) = session_agent(
            dir.path().to_path_buf(),
            json!({}),
            false,
            old_session("fake"),
            earlier(),
        );
        let (events, prompt) = run_prompt(&runtime, &seen, "next");
        assert_eq!(
            methods(&seen),
            ["initialize", "session/new", "session/prompt"]
        );
        assert_eq!(opened(&events), ["fake:fresh"]);
        assert_eq!(prompt.len(), 2);
        let recap = prompt[0]["text"].as_str().unwrap();
        assert!(recap.contains("earlier request") && recap.contains("earlier answer"));
        assert_eq!(prompt[1]["text"], "next");
        // The recap is not part of what the panel logs as the request.
        let logged = events.iter().find_map(|e| match e {
            AgentEvent::MessageEnd(Message::User(user)) => Some(user.plain_text()),
            _ => None,
        });
        assert_eq!(logged.as_deref(), Some("next"));

        seen.lock().unwrap().clear();
        let (_, prompt) = run_prompt(&runtime, &seen, "again");
        assert_eq!(prompt, [json!({ "type": "text", "text": "again" })]);
    }

    #[test]
    fn a_session_the_agent_lost_or_never_had_starts_anew_with_the_recap() {
        for (forgotten, resume) in [(true, old_session("fake")), (false, old_session("other"))] {
            let dir = tempfile::tempdir().unwrap();
            let (runtime, seen) = session_agent(
                dir.path().to_path_buf(),
                json!({ "sessionCapabilities": { "resume": {} } }),
                forgotten,
                resume,
                earlier(),
            );
            let (events, prompt) = run_prompt(&runtime, &seen, "next");
            assert!(methods(&seen).contains(&"session/new".to_string()));
            assert_eq!(
                methods(&seen).contains(&"session/resume".to_string()),
                forgotten,
                "another agent's session is never asked for"
            );
            assert_eq!(opened(&events), ["fake:fresh"]);
            assert_eq!(prompt.len(), 2);
        }
    }

    #[test]
    fn a_fresh_conversation_gets_no_recap() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) =
            session_agent(dir.path().to_path_buf(), json!({}), false, None, Vec::new());
        let (events, prompt) = run_prompt(&runtime, &seen, "first");
        assert_eq!(opened(&events), ["fake:fresh"]);
        assert_eq!(prompt, [json!({ "type": "text", "text": "first" })]);
    }
}
