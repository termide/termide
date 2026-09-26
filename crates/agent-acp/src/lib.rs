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

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::{
    expand_env, now_millis, AcpConfig, AcpFlavor, Agent, AgentEvent, AssistantContent,
    AssistantMessage, Backend, BackendModel, BackendSetup, CancelToken, Hooks, HostTools, Message,
    Mode, ModeHandle, PermissionHooks, PromptError, StopReason, StreamEvent, ToolCall, ToolContext,
    ToolDecision, ToolResultMessage, Usage, UserMessage,
};
use termide_agent_mcp::{McpServer, SERVER_NAME};

/// How the calls of termide's tools reach the transcript from Claude Code:
/// named after the MCP server that serves them.
const HOST_TOOL_PREFIX: &str = "mcp__termide__";

/// The protocol version requested.
pub const PROTOCOL_VERSION: u64 = 1;

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
    /// Which adapter this is.
    flavor: AcpFlavor,
    /// termide's system prompt, for an adapter that takes it.
    system_prompt: String,
    /// termide's tools, until the handshake serves them.
    host_tools: Mutex<Option<HostTools>>,
    /// The server of termide's tools, alive as long as the agent.
    mcp_server: Mutex<Option<McpServer>>,
    /// Calls announced without their arguments yet, by id.
    announced: Mutex<HashMap<String, Value>>,
    /// The panel's live permission mode.
    mode: ModeHandle,
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
        let runtime = Self::from_streams(
            name,
            stdout,
            stdin,
            setup,
            config.timeout_secs,
            config.flavor,
        );
        *runtime.shared.child.lock().unwrap() = Some(child);
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
        let (events_tx, events) = mpsc::channel();
        let shared = Arc::new(Shared {
            writer: Arc::new(Mutex::new(Some(Box::new(writer)))),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            events: events_tx,
            conn: Mutex::new(Conn::Starting),
            queue: Mutex::new(Vec::new()),
            busy: AtomicBool::new(false),
            cancel: setup.cancel,
            hooks: Mutex::new({
                let mut hooks = PermissionHooks::new(setup.rules, Box::new(setup.prompter))
                    .with_mode_handle(setup.mode.clone());
                if let Some(persist) = setup.persist {
                    hooks = hooks.with_persist(persist);
                }
                hooks
            }),
            cwd: setup.cwd,
            name: name.to_string(),
            open_message: Mutex::new(None),
            child: Mutex::new(None),
            models: Mutex::new(Vec::new()),
            current_model: Mutex::new(None),
            model_option: Mutex::new(None),
            flavor,
            system_prompt: setup.system_prompt,
            host_tools: Mutex::new(setup.host_tools),
            mcp_server: Mutex::new(None),
            announced: Mutex::new(HashMap::new()),
            mode: setup.mode,
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
        if matches!(*self.shared.conn.lock().unwrap(), Conn::Failed(_)) {
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
        self.shared.queue.lock().unwrap().insert(0, message);
        self.shared.kick();
        Ok(())
    }

    fn steer(&self, message: UserMessage) {
        self.shared.queue.lock().unwrap().push(message);
        let lens = self.queue_lens();
        let _ = self.shared.events.send(AgentEvent::QueueUpdate {
            steering: lens.0,
            follow_up: lens.1,
        });
    }

    fn take_queued(&self) -> Vec<UserMessage> {
        let mut queue = self.shared.queue.lock().unwrap();
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
        let queued = self.shared.queue.lock().unwrap().len();
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
        if let Conn::Ready { session_id } = &*self.shared.conn.lock().unwrap() {
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

    fn available_models(&self) -> Vec<BackendModel> {
        self.shared.models.lock().unwrap().clone()
    }

    fn current_model(&self) -> Option<String> {
        self.shared.current_model.lock().unwrap().clone()
    }

    /// Claude Code's calls of termide's tools, and its permission requests,
    /// are judged here; Codex's modes are mapped from the panel's. Another
    /// agent answers to its own configuration.
    fn follows_mode(&self) -> bool {
        self.shared.flavor != AcpFlavor::Generic
    }

    fn set_mode(&self, mode: Mode) {
        // The hooks read the shared handle; Codex is told, off the UI thread.
        if self.shared.flavor == AcpFlavor::Codex {
            let shared = Arc::clone(&self.shared);
            std::thread::spawn(move || shared.apply_mode(mode));
        }
    }

    fn select_model(&self, model_id: String) -> Result<(), String> {
        let session_id = match &*self.shared.conn.lock().unwrap() {
            Conn::Ready { session_id } => session_id.clone(),
            Conn::Starting => return Err("the agent is still starting".to_string()),
            Conn::Failed(error) => return Err(error.clone()),
        };
        // Refused while a turn runs: the switch applies to the runs that follow,
        // like the built-in loop's model change between turns.
        if self.is_busy() {
            return Err("finish or stop the current task first".to_string());
        }
        let option = self.shared.model_option.lock().unwrap().clone();
        match option {
            Some(config_id) => self.shared.request(
                "session/set_config_option",
                json!({ "sessionId": session_id, "configId": config_id, "value": model_id }),
                Duration::from_secs(30),
            )?,
            None => self.shared.request(
                "session/set_model",
                json!({ "sessionId": session_id, "modelId": model_id }),
                Duration::from_secs(30),
            )?,
        };
        *self.shared.current_model.lock().unwrap() = Some(model_id);
        Ok(())
    }

    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

impl Drop for AcpRuntime {
    fn drop(&mut self) {
        self.shared.cancel.cancel();
        // Closing stdin tells a well-behaved agent to exit; kill the rest.
        self.shared.writer.lock().unwrap().take();
        if let Some(mut child) = self.shared.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Shared {
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
        let params = self.new_session_params();
        let result = outcome.and_then(|_| self.request("session/new", params, timeout));
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
        *self.conn.lock().unwrap() = conn;
        if ready && self.flavor == AcpFlavor::Codex {
            self.apply_mode(self.mode.get());
        }
        self.kick();
    }

    /// `session/new`'s parameters. Claude Code gets termide's tools from
    /// termide's MCP server in place of its own, termide's system prompt in
    /// place of its own, none of its own settings (their rules, hooks,
    /// `CLAUDE.md`, MCP servers), and leave to run termide's tools without
    /// asking, since termide judges each call as it runs it. Without the
    /// server it keeps its own tools, so it still has some.
    fn new_session_params(&self) -> Value {
        let mut params = json!({ "cwd": self.cwd, "mcpServers": [] });
        if self.flavor != AcpFlavor::ClaudeCode {
            return params;
        }
        let mut options = json!({ "settingSources": [], "strictMcpConfig": true });
        if let Some(host) = self.host_tools.lock().unwrap().take() {
            match McpServer::start(host.tools, host.hooks, self.cwd.clone()) {
                Ok(server) => {
                    params["mcpServers"] = json!([{
                        "type": "http",
                        "name": SERVER_NAME,
                        "url": server.url(),
                        "headers": [
                            { "name": "Authorization", "value": format!("Bearer {}", server.token()) }
                        ],
                    }]);
                    options["tools"] = json!([]);
                    options["allowedTools"] = json!([format!("mcp__{SERVER_NAME}")]);
                    *self.mcp_server.lock().unwrap() = Some(server);
                }
                Err(error) => log::warn!("cannot serve termide's tools to {}: {error}", self.name),
            }
        }
        params["_meta"] = json!({
            "systemPrompt": self.system_prompt,
            "claudeCode": { "options": options },
        });
        params
    }

    /// Put Codex in the modes that match the panel's `mode`: its approval
    /// preset, and its plan collaboration mode for `plan`. `ask` and
    /// `configured` have it ask about everything, so termide's rules and the
    /// user decide.
    fn apply_mode(&self, mode: Mode) {
        let session_id = match &*self.conn.lock().unwrap() {
            Conn::Ready { session_id } => session_id.clone(),
            _ => return,
        };
        let (approval, collaboration) = codex_modes(mode);
        for (config_id, value) in [("mode", approval), ("collaboration_mode", collaboration)] {
            let set = self.request(
                "session/set_config_option",
                json!({ "sessionId": session_id, "configId": config_id, "value": value }),
                Duration::from_secs(30),
            );
            if let Err(error) = set {
                log::warn!(
                    "acp {}: cannot set {config_id} to {value}: {error}",
                    self.name
                );
            }
        }
    }

    /// Record the models an agent advertises in a `session/new`/`load` result,
    /// so the panel can list and switch them: `models` (`availableModels`,
    /// `currentModelId`) when it has them, else the `model` entry of its
    /// `configOptions`. Missing or malformed data leaves the lists empty.
    fn adopt_models(&self, result: &Value) {
        if result["models"].is_object() {
            *self.model_option.lock().unwrap() = None;
            self.adopt_model_list(&result["models"]);
        } else {
            self.adopt_model_option(&result["configOptions"]);
        }
    }

    /// The `model` config option: a select whose options are the models,
    /// flat or in groups, and whose current value is the model in use.
    fn adopt_model_option(&self, options: &Value) {
        let Some(option) = options.as_array().and_then(|options| {
            options
                .iter()
                .find(|o| o["category"] == "model" || o["id"] == "model")
        }) else {
            return;
        };
        let Some(config_id) = option["id"].as_str() else {
            return;
        };
        let entry = |o: &Value| {
            let id = o["value"].as_str()?.to_string();
            let name = o["name"].as_str().unwrap_or(&id).to_string();
            Some(BackendModel { id, name })
        };
        let list: Vec<BackendModel> = option["options"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|o| match o["options"].as_array() {
                Some(group) => group.iter().filter_map(entry).collect::<Vec<_>>(),
                None => entry(o).into_iter().collect(),
            })
            .collect();
        *self.model_option.lock().unwrap() = Some(config_id.to_string());
        *self.models.lock().unwrap() = list;
        if let Some(current) = option["currentValue"].as_str() {
            *self.current_model.lock().unwrap() = Some(current.to_string());
        }
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
        *self.models.lock().unwrap() = list;
        if let Some(current) = current {
            *self.current_model.lock().unwrap() = Some(current);
        }
    }

    /// Start the next queued turn when the session is ready and no turn is
    /// running; report a failed connection as a failed answer.
    fn kick(self: &Arc<Self>) {
        if !self.busy.load(Ordering::Acquire) {
            return;
        }
        let session_id = match &*self.conn.lock().unwrap() {
            Conn::Starting => return,
            Conn::Ready { session_id } => session_id.clone(),
            Conn::Failed(error) => {
                self.queue.lock().unwrap().clear();
                let _ = self.events.send(AgentEvent::MessageEnd(Message::Assistant(
                    AssistantMessage::failed("acp", &self.name, StopReason::Error, error.clone()),
                )));
                let _ = self.events.send(AgentEvent::AgentEnd);
                self.busy.store(false, Ordering::Release);
                return;
            }
        };
        // Everything queued goes as one turn: messages typed while the agent
        // works are usually one thought written in pieces.
        let message = UserMessage::merge(self.queue.lock().unwrap().drain(..).collect());
        let Some(message) = message else {
            let _ = self.events.send(AgentEvent::AgentEnd);
            self.busy.store(false, Ordering::Release);
            return;
        };
        let this = Arc::clone(self);
        std::thread::spawn(move || this.run_turn(&session_id, message));
    }

    fn run_turn(self: &Arc<Self>, session_id: &str, message: UserMessage) {
        let _ = self.events.send(AgentEvent::TurnStart);
        let _ = self
            .events
            .send(AgentEvent::MessageEnd(Message::User(message.clone())));
        let text = message.plain_text();
        let result = self.request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": text }]
            }),
            Duration::from_secs(60 * 60 * 24),
        );
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
        self.close_message_with(stop, error, usage);
        let _ = self.events.send(AgentEvent::TurnEnd);
        if self.cancel.is_cancelled() {
            self.queue.lock().unwrap().clear();
        }
        let queued = self.queue.lock().unwrap().len();
        let _ = self.events.send(AgentEvent::QueueUpdate {
            steering: queued,
            follow_up: 0,
        });
        self.kick();
    }

    /// Finish the assistant message being streamed, or make one for an
    /// error, so the transcript and the log get a complete message.
    fn close_message(&self, stop: StopReason, error: Option<String>) {
        self.close_message_with(stop, error, Usage::default());
    }

    /// [`Self::close_message`], with the turn's token usage when the agent
    /// reported it.
    fn close_message_with(&self, stop: StopReason, error: Option<String>, usage: Usage) {
        let text = self.open_message.lock().unwrap().take();
        if text.is_none() && error.is_none() {
            return;
        }
        let message = AssistantMessage {
            content: text
                .map(|text| vec![AssistantContent::Text { text }])
                .unwrap_or_default(),
            stop_reason: stop,
            usage,
            provider: "acp".into(),
            model: self
                .current_model
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| self.name.clone()),
            error_message: error,
            timestamp: now_millis(),
        };
        let _ = self
            .events
            .send(AgentEvent::MessageEnd(Message::Assistant(message)));
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(error) = self.write(&message) {
            self.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.pending.lock().unwrap().remove(&id);
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
        let mut guard = self.writer.lock().unwrap();
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
                    let reply = match message.get("error") {
                        Some(error) => Err(format!(
                            "{} (code {})",
                            error["message"].as_str().unwrap_or("error"),
                            error["code"]
                        )),
                        None => Ok(message["result"].clone()),
                    };
                    if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(reply);
                    }
                }
                (Some(id), Some(method)) => {
                    let method = method.as_str().unwrap_or("").to_string();
                    self.serve_request(id, &method, &message["params"]);
                }
                (None, Some(method)) => {
                    if method == "session/update" {
                        self.on_update(&message["params"]["update"]);
                    } else {
                        log::debug!("acp {}: notification {method}", self.name);
                    }
                }
                (None, None) => {}
            }
        }
        // The agent is gone: a pending prompt learns it, later ones are refused.
        self.pending.lock().unwrap().clear();
        let mut conn = self.conn.lock().unwrap();
        if !matches!(*conn, Conn::Failed(_)) {
            *conn = Conn::Failed("the agent exited".into());
        }
    }

    /// Answer the agent's requests: permissions through the user, files
    /// from the working directory, nothing else.
    fn serve_request(self: &Arc<Self>, id: u64, method: &str, params: &Value) {
        match method {
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
        let call = permission_call(&params["toolCall"]);
        let ctx = ToolContext {
            cwd: self.cwd.clone(),
        };
        // The hooks decide the request as they would for the built-in agent:
        // a read-only command or a matching rule passes without a prompt, an
        // unknown one reaches the user, and a session or always grant is
        // recorded so it is asked only once. The user is never troubled with
        // anything the built-in agent would have let through silently.
        let decision = self.hooks.lock().unwrap().before_tool_call(&call, &ctx);
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
        // Always answer "once": termide stays the source of truth for grants,
        // so the agent keeps asking and termide keeps deciding silently,
        // rather than the agent remembering a rule of its own.
        let chosen = match decision {
            ToolDecision::Block { .. } => pick(&["reject_once", "reject_always"]),
            // The permission hooks only ever allow or block; any allowing
            // verdict answers the request "once".
            _ => pick(&["allow_once", "allow_always"]),
        };
        match chosen {
            Some(option_id) => {
                json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
            }
            None => json!({ "outcome": { "outcome": "cancelled" } }),
        }
    }

    /// One `session/update` into the events the panel understands.
    fn on_update(&self, update: &Value) {
        match update["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => {
                let text = update["content"]["text"].as_str().unwrap_or("").to_string();
                let mut open = self.open_message.lock().unwrap();
                if open.is_none() {
                    *open = Some(String::new());
                    let _ = self.events.send(AgentEvent::MessageStart);
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
                if self.open_message.lock().unwrap().is_none() {
                    *self.open_message.lock().unwrap() = Some(String::new());
                    let _ = self.events.send(AgentEvent::MessageStart);
                }
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
                    self.announced.lock().unwrap().insert(id, update.clone());
                    return;
                }
                let _ = self.events.send(AgentEvent::ToolExecutionStart {
                    call: tool_call_of(update),
                });
                if is_finished(update) {
                    self.finish_tool_call(update);
                }
            }
            "tool_call_update" => {
                let id = update["toolCallId"].as_str().unwrap_or("");
                if has_arguments(update) || is_finished(update) {
                    let announced = self.announced.lock().unwrap().remove(id);
                    if let Some(mut call) = announced {
                        for (key, value) in update.as_object().into_iter().flatten() {
                            if !value.is_null() {
                                call[key] = value.clone();
                            }
                        }
                        let _ = self.events.send(AgentEvent::ToolExecutionStart {
                            call: tool_call_of(&call),
                        });
                    }
                }
                if is_finished(update) {
                    self.finish_tool_call(update);
                }
            }
            "current_model_update" => {
                if let Some(id) = update["modelId"].as_str() {
                    *self.current_model.lock().unwrap() = Some(id.to_string());
                }
            }
            // The agent changed its options itself: the model among them.
            "config_option_update" if self.model_option.lock().unwrap().is_some() => {
                self.adopt_model_option(&update["configOptions"]);
            }
            other => log::debug!("acp {}: update {other} ignored", self.name),
        }
    }

    fn finish_tool_call(&self, update: &Value) {
        let call = tool_call_of(update);
        let text = content_text(&update["content"]);
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
        if let Some(path) = update["locations"][0]["path"].as_str() {
            result = result.with_details(json!({ "path": absolute(&self.cwd, path) }));
        }
        let _ = self.events.send(AgentEvent::ToolExecutionEnd { result });
    }
}

/// The ACP tool call as the panel's [`ToolCall`]: the kind stands as the
/// name (`edit`, `execute`, `read`, …), so the transcript line and the
/// editor reload behave as for built-in tools, and the title travels in the
/// arguments when the agent gives no raw input.
fn tool_call_of(update: &Value) -> ToolCall {
    let title = update["title"].as_str().unwrap_or("").to_string();
    let mut arguments = update
        .get("rawInput")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    // A call of termide's own tool shows as that tool, as the built-in loop
    // shows it.
    if let Some(name) = title.strip_prefix(HOST_TOOL_PREFIX) {
        return ToolCall {
            id: update["toolCallId"].as_str().unwrap_or("").to_string(),
            name: name.to_string(),
            arguments,
        };
    }
    let kind = update["kind"].as_str().unwrap_or("other").to_string();
    if !title.is_empty() {
        arguments["title"] = json!(title);
    }
    ToolCall {
        id: update["toolCallId"].as_str().unwrap_or("").to_string(),
        name: kind,
        arguments,
    }
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
fn codex_modes(mode: Mode) -> (&'static str, &'static str) {
    match mode {
        Mode::Ask | Mode::Configured => ("read-only", "default"),
        Mode::Plan => ("read-only", "plan"),
        Mode::Edit => ("agent", "default"),
        Mode::All => ("agent-full-access", "default"),
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
                host_tools: None,
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
    /// handshake and `session/set_config_option`.
    fn recording_agent(
        dir: PathBuf,
        flavor: AcpFlavor,
        host_tools: Option<HostTools>,
        mode: ModeHandle,
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
                    Some("initialize") => json!({ "protocolVersion": 1, "agentCapabilities": {} }),
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
                host_tools,
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
            ToolResultMessage::text(call, "echoed")
        }
    }

    #[test]
    fn claude_code_gets_termides_prompt_and_tools_and_none_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let mut tools = termide_agent_core::ToolRegistry::new();
        tools.insert(Arc::new(Echo));
        let host = HostTools {
            tools,
            hooks: Box::new(termide_agent_core::NoHooks),
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
        runtime.set_mode(Mode::All);
        seen_where(&seen, option("mode", "agent-full-access"));
        runtime.set_mode(Mode::Plan);
        seen_where(&seen, option("collaboration_mode", "plan"));
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
                host_tools: None,
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

        let kinds: Vec<String> = events
            .iter()
            .map(|e| match e {
                AgentEvent::AgentStart => "start".into(),
                AgentEvent::TurnStart => "turn".into(),
                AgentEvent::MessageStart => "msg-start".into(),
                AgentEvent::MessageUpdate(StreamEvent::TextDelta(t)) => format!("text:{t}"),
                AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(_)) => "think".into(),
                AgentEvent::MessageUpdate(_) => "update".into(),
                AgentEvent::MessageEnd(Message::User(u)) => format!("user:{}", u.plain_text()),
                AgentEvent::MessageEnd(Message::Assistant(a)) => {
                    format!("assistant:{}:{:?}", a.plain_text(), a.stop_reason)
                }
                AgentEvent::MessageEnd(Message::ToolResult(_)) => "tool-result-msg".into(),
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
                _ => "other".into(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "start",
                "turn",
                "user:first",
                "msg-start",
                "think",
                "text:Editing ",
                "text:now.",
                "assistant:Editing now.:ToolUse",
                "tool-start:edit:\"Write notes.md\"",
                "tool-start:write:null",
                "tool-end:write:true",
                "tool-end:edit:true",
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
                host_tools: None,
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
        let ctx = ToolContext {
            cwd: PathBuf::from("/tmp"),
        };
        assert_eq!(hooks.before_tool_call(&call, &ctx), ToolDecision::Allow);
        assert!(
            rx.try_recv().is_err(),
            "a read-only command must not reach the user"
        );
    }
}
