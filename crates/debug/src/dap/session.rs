//! A debug session over DAP: the adapter process, the requests in flight and
//! what the user sees of the debuggee — whether it runs, where it stopped,
//! its call stack, variables and output.
//!
//! Nothing here blocks: the adapter's messages arrive on a channel from a
//! reader thread, and [`Session::poll`] applies them. Requests go out as the
//! user acts; each one remembers what its response is for.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::adapter::Adapter;
use super::transport::{encode, read_message};
use crate::Breakpoint;

/// Lines of output kept; older ones are dropped.
const MAX_OUTPUT_LINES: usize = 5000;
/// Frames asked for at a stop.
const STACK_DEPTH: u64 = 64;

/// What to debug, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub adapter: Adapter,
    /// The program — for debugpy, the script.
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// A shell command run first; the adapter starts only if it succeeds.
    pub build: Option<String>,
    /// Environment of the build and the adapter.
    pub env: Vec<(String, String)>,
    /// What the session is called in the panel.
    pub label: String,
}

/// Where the session stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The build command runs.
    Building,
    /// The adapter starts and launches the program.
    Starting,
    Running,
    Stopped {
        reason: String,
        description: Option<String>,
    },
    /// The program exited; the session is over.
    Exited {
        code: Option<i64>,
    },
    /// The session ended without an exit code: stopped by the user, or the
    /// adapter went away.
    Ended,
    Failed(String),
}

impl Status {
    /// Whether the session still runs or can still be resumed.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Building | Self::Starting | Self::Running | Self::Stopped { .. }
        )
    }
}

/// One frame of the call stack, innermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// 1-based.
    pub line: usize,
}

/// A scope or a variable; one with a non-zero `reference` has children,
/// fetched when it is first expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: Option<String>,
    pub reference: i64,
    pub children: Option<Vec<Variable>>,
    pub expanded: bool,
    /// A scope the adapter calls expensive to fetch, left closed.
    pub expensive: bool,
}

impl Variable {
    pub fn has_children(&self) -> bool {
        self.reference > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Stdout,
    Stderr,
    /// The adapter's own messages.
    Console,
    Build,
    /// An expression the user asked to evaluate.
    Input,
    /// Its value.
    Result,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub kind: OutputKind,
    pub text: String,
}

/// What a response that is still to come is for.
#[derive(Debug, Clone)]
enum Pending {
    Initialize,
    Launch,
    SetBreakpoints,
    ConfigurationDone,
    /// Threads at `generation`; `then_stack` asks for the stack of the first.
    Threads {
        generation: u64,
        then_stack: bool,
    },
    StackTrace {
        generation: u64,
    },
    Scopes {
        generation: u64,
        frame_id: i64,
    },
    Variables {
        generation: u64,
        path: Vec<usize>,
    },
    Evaluate,
    /// continue, next, stepIn, stepOut, pause.
    Control,
    Disconnect,
}

enum Incoming {
    Message(Value),
    Closed,
    Log(String),
    BuildLine(String),
    BuildDone(bool),
}

/// A debug session; see the module documentation.
pub struct Session {
    spec: LaunchSpec,
    breakpoints: Vec<Breakpoint>,
    pub status: Status,
    pub frames: Vec<Frame>,
    pub selected_frame: usize,
    /// The scopes of the selected frame, each a tree of variables.
    pub scopes: Vec<Variable>,
    pub output: VecDeque<OutputLine>,
    /// Bumped by every change the panel should show.
    pub version: u64,

    exit_code: Option<i64>,
    thread_id: Option<i64>,
    /// Bumped at every stop and resume; a response asked for before it is
    /// stale and dropped.
    generation: u64,
    capabilities: Value,
    configured: bool,
    /// Lines last sent per file, to send only changes.
    sent: BTreeMap<PathBuf, Vec<usize>>,
    seq: i64,
    pending: HashMap<i64, Pending>,
    partial: Option<(OutputKind, String)>,

    writer: Option<Box<dyn Write + Send>>,
    inbox: Receiver<Incoming>,
    inbox_tx: Sender<Incoming>,
    adapter: Option<Child>,
    build: Option<Arc<Mutex<Child>>>,
}

impl Session {
    /// Start a session: run the build, if any, then the adapter, which
    /// launches the program with `breakpoints` set. A failure shows as the
    /// session's status.
    pub fn start(spec: LaunchSpec, breakpoints: Vec<Breakpoint>) -> Self {
        let (inbox_tx, inbox) = mpsc::channel();
        let mut session = Self::new(spec, breakpoints, inbox, inbox_tx);
        match session.spec.build.clone().filter(|b| !b.trim().is_empty()) {
            Some(build) => session.start_build(&build),
            None => session.start_adapter(),
        }
        session
    }

    /// A session with no adapter behind it, whose state is set by hand: for
    /// tests of what shows it.
    pub fn detached(spec: LaunchSpec) -> Self {
        let (inbox_tx, inbox) = mpsc::channel();
        Self::new(spec, Vec::new(), inbox, inbox_tx)
    }

    fn new(
        spec: LaunchSpec,
        breakpoints: Vec<Breakpoint>,
        inbox: Receiver<Incoming>,
        inbox_tx: Sender<Incoming>,
    ) -> Self {
        let breakpoints = breakpoints
            .into_iter()
            .filter(|b| spec.adapter.accepts(&b.path))
            .collect();
        Self {
            spec,
            breakpoints,
            status: Status::Starting,
            frames: Vec::new(),
            selected_frame: 0,
            scopes: Vec::new(),
            output: VecDeque::new(),
            version: 0,
            exit_code: None,
            thread_id: None,
            generation: 0,
            capabilities: Value::Null,
            configured: false,
            sent: BTreeMap::new(),
            seq: 0,
            pending: HashMap::new(),
            partial: None,
            writer: None,
            inbox,
            inbox_tx,
            adapter: None,
            build: None,
        }
    }

    pub fn spec(&self) -> &LaunchSpec {
        &self.spec
    }

    pub fn is_active(&self) -> bool {
        self.status.is_active()
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self.status, Status::Stopped { .. })
    }

    /// The source line of the selected frame while the program is stopped.
    pub fn execution_point(&self) -> Option<(PathBuf, usize)> {
        if !self.is_stopped() {
            return None;
        }
        let frame = self.frames.get(self.selected_frame)?;
        Some((frame.path.clone()?, frame.line))
    }

    // === Starting ===

    fn start_build(&mut self, build: &str) {
        self.status = Status::Building;
        self.push_line(OutputKind::Build, format!("$ {build}"));
        let mut command = shell_command(build);
        command
            .current_dir(&self.spec.cwd)
            .envs(self.spec.env.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => return self.fail(format!("{build}: {e}")),
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        for stream in [
            stdout.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            stderr.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let tx = self.inbox_tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if tx.send(Incoming::BuildLine(line)).is_err() {
                        break;
                    }
                }
            });
        }
        let child = Arc::new(Mutex::new(child));
        self.build = Some(Arc::clone(&child));
        let tx = self.inbox_tx.clone();
        std::thread::spawn(move || loop {
            let status = child.lock().unwrap_or_else(|e| e.into_inner()).try_wait();
            match status {
                Ok(Some(status)) => {
                    let _ = tx.send(Incoming::BuildDone(status.success()));
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => {
                    let _ = tx.send(Incoming::BuildDone(false));
                    break;
                }
            }
        });
    }

    fn start_adapter(&mut self) {
        self.status = Status::Starting;
        let Some((program, args)) = self.spec.adapter.command() else {
            let name = self.spec.adapter.as_str();
            return self.fail(format!("{name} is not installed"));
        };
        let mut command = Command::new(&program);
        command
            .args(&args)
            .current_dir(&self.spec.cwd)
            .envs(self.spec.env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => return self.fail(format!("{}: {e}", program.display())),
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return self.fail("the adapter's pipes did not open".into());
        };
        let tx = self.inbox_tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(message)) = read_message(&mut reader) {
                if tx.send(Incoming::Message(message)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Incoming::Closed);
        });
        let tx = self.inbox_tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(Incoming::Log(line)).is_err() {
                    break;
                }
            }
        });
        self.adapter = Some(child);
        self.connect(Box::new(stdin));
    }

    /// Talk to the adapter through `writer`: start with `initialize`.
    fn connect(&mut self, writer: Box<dyn Write + Send>) {
        self.writer = Some(writer);
        self.request(
            "initialize",
            json!({
                "clientID": "termide",
                "clientName": "TermIDE",
                "adapterID": self.spec.adapter.id(),
                "locale": "en",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path",
                "supportsVariableType": true,
                "supportsRunInTerminalRequest": false,
            }),
            Pending::Initialize,
        );
    }

    fn fail(&mut self, message: String) {
        log::warn!("Debug session: {message}");
        self.push_line(OutputKind::Error, message.clone());
        self.status = Status::Failed(message);
        self.changed();
    }

    // === The wire ===

    fn send(&mut self, message: Value) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        let bytes = encode(&message);
        if writer
            .write_all(&bytes)
            .and_then(|_| writer.flush())
            .is_err()
        {
            log::warn!("Debug session: the adapter stopped reading");
            self.writer = None;
        }
    }

    fn request(&mut self, command: &str, arguments: Value, pending: Pending) {
        if self.writer.is_none() {
            return;
        }
        self.seq += 1;
        let seq = self.seq;
        self.pending.insert(seq, pending);
        self.send(json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        }));
    }

    /// Apply what arrived from the adapter and the build. Returns whether
    /// anything changed.
    pub fn poll(&mut self) -> bool {
        let before = self.version;
        while let Ok(incoming) = self.inbox.try_recv() {
            match incoming {
                Incoming::Message(message) => self.handle_message(message),
                Incoming::Log(line) => {
                    log::debug!("Debug adapter: {line}");
                    self.push_line(OutputKind::Console, line);
                }
                Incoming::BuildLine(line) => self.push_line(OutputKind::Build, line),
                Incoming::BuildDone(success) => {
                    self.build = None;
                    if self.status != Status::Building {
                        continue;
                    }
                    if success {
                        self.start_adapter();
                    } else {
                        self.fail("the build failed".into());
                    }
                }
                Incoming::Closed => {
                    self.writer = None;
                    self.flush_partial();
                    if self.is_active() {
                        self.status = match self.status {
                            Status::Starting => Status::Failed("the debug adapter exited".into()),
                            _ => self.ended_status(),
                        };
                    }
                    self.changed();
                }
            }
        }
        self.version != before
    }

    fn changed(&mut self) {
        self.version += 1;
    }

    fn ended_status(&self) -> Status {
        match self.exit_code {
            Some(code) => Status::Exited { code: Some(code) },
            None => Status::Ended,
        }
    }

    fn handle_message(&mut self, message: Value) {
        match message["type"].as_str() {
            Some("response") => self.handle_response(message),
            Some("event") => self.handle_event(message),
            Some("request") => {
                // Reverse requests (runInTerminal, startDebugging) are not
                // supported; say so rather than leave the adapter waiting.
                let seq = message["seq"].as_i64().unwrap_or_default();
                let command = message["command"].as_str().unwrap_or_default().to_string();
                self.seq += 1;
                let reply = json!({
                    "seq": self.seq,
                    "type": "response",
                    "request_seq": seq,
                    "command": command,
                    "success": false,
                    "message": "not supported by termide",
                });
                self.send(reply);
            }
            _ => {}
        }
    }

    fn handle_response(&mut self, message: Value) {
        let Some(seq) = message["request_seq"].as_i64() else {
            return;
        };
        let Some(pending) = self.pending.remove(&seq) else {
            return;
        };
        let body = &message["body"];
        if !message["success"].as_bool().unwrap_or(false) {
            let text = body["error"]["format"]
                .as_str()
                .or_else(|| message["message"].as_str())
                .unwrap_or("request failed")
                .to_string();
            match pending {
                Pending::Initialize | Pending::Launch => self.fail(text),
                Pending::Evaluate => {
                    self.push_line(OutputKind::Error, text);
                    self.changed();
                }
                Pending::Disconnect => {}
                _ => log::debug!("Debug session: {text}"),
            }
            return;
        }
        match pending {
            Pending::Initialize => {
                self.capabilities = body.clone();
                let arguments = self.spec.adapter.launch_arguments(
                    &self.spec.program,
                    &self.spec.args,
                    &self.spec.cwd,
                );
                self.request("launch", arguments, Pending::Launch);
            }
            Pending::Launch | Pending::ConfigurationDone => {
                if self.status == Status::Starting {
                    self.status = Status::Running;
                    self.changed();
                }
            }
            Pending::SetBreakpoints | Pending::Control | Pending::Disconnect => {}
            Pending::Threads {
                generation,
                then_stack,
            } => {
                let first = body["threads"]
                    .as_array()
                    .and_then(|t| t.first())
                    .and_then(|t| t["id"].as_i64());
                if self.thread_id.is_none() {
                    self.thread_id = first;
                }
                if then_stack && generation == self.generation {
                    self.request_stack();
                }
            }
            Pending::StackTrace { generation } => {
                if generation != self.generation {
                    return;
                }
                self.frames = body["stackFrames"]
                    .as_array()
                    .map(|frames| frames.iter().map(parse_frame).collect())
                    .unwrap_or_default();
                self.selected_frame = 0;
                self.request_scopes();
                self.changed();
            }
            Pending::Scopes {
                generation,
                frame_id,
            } => {
                let current = self.frames.get(self.selected_frame).map(|f| f.id);
                if generation != self.generation || current != Some(frame_id) {
                    return;
                }
                self.scopes = body["scopes"]
                    .as_array()
                    .map(|scopes| {
                        scopes
                            .iter()
                            .map(|s| Variable {
                                name: s["name"].as_str().unwrap_or_default().to_string(),
                                value: String::new(),
                                type_name: None,
                                reference: s["variablesReference"].as_i64().unwrap_or_default(),
                                children: None,
                                expanded: false,
                                expensive: s["expensive"].as_bool().unwrap_or(false),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(first) = self.scopes.iter().position(|s| !s.expensive) {
                    self.toggle_variable(&[first]);
                }
                self.changed();
            }
            Pending::Variables { generation, path } => {
                if generation != self.generation {
                    return;
                }
                let children: Vec<Variable> = body["variables"]
                    .as_array()
                    .map(|vars| vars.iter().map(parse_variable).collect())
                    .unwrap_or_default();
                if let Some(node) = node_mut(&mut self.scopes, &path) {
                    node.children = Some(children);
                    node.expanded = true;
                }
                self.changed();
            }
            Pending::Evaluate => {
                let result = body["result"].as_str().unwrap_or_default().to_string();
                self.push_line(OutputKind::Result, result);
                self.changed();
            }
        }
    }

    fn handle_event(&mut self, message: Value) {
        let body = &message["body"];
        match message["event"].as_str().unwrap_or_default() {
            "initialized" => {
                let files = group_by_file(&self.breakpoints);
                for (path, lines) in &files {
                    self.send_breakpoints(path, lines);
                }
                self.sent = files;
                self.configured = true;
                if self.capabilities["supportsConfigurationDoneRequest"]
                    .as_bool()
                    .unwrap_or(false)
                {
                    self.request("configurationDone", json!({}), Pending::ConfigurationDone);
                }
                let generation = self.generation;
                self.request(
                    "threads",
                    json!({}),
                    Pending::Threads {
                        generation,
                        then_stack: false,
                    },
                );
            }
            "stopped" => {
                self.generation += 1;
                self.status = Status::Stopped {
                    reason: body["reason"].as_str().unwrap_or("pause").to_string(),
                    description: body["description"]
                        .as_str()
                        .or_else(|| body["text"].as_str())
                        .map(str::to_string),
                };
                self.frames.clear();
                self.scopes.clear();
                match body["threadId"].as_i64() {
                    Some(thread) => {
                        self.thread_id = Some(thread);
                        self.request_stack();
                    }
                    None => {
                        let generation = self.generation;
                        self.request(
                            "threads",
                            json!({}),
                            Pending::Threads {
                                generation,
                                then_stack: true,
                            },
                        );
                    }
                }
                self.changed();
            }
            "continued" => {
                if self.is_stopped() {
                    self.resumed();
                }
            }
            "exited" => {
                self.exit_code = body["exitCode"].as_i64();
            }
            "terminated" => {
                self.flush_partial();
                if self.is_active() {
                    self.status = self.ended_status();
                }
                self.frames.clear();
                self.scopes.clear();
                self.request(
                    "disconnect",
                    json!({ "terminateDebuggee": true }),
                    Pending::Disconnect,
                );
                self.changed();
            }
            "output" => {
                let kind = match body["category"].as_str().unwrap_or("console") {
                    "stdout" => OutputKind::Stdout,
                    "stderr" => OutputKind::Stderr,
                    "telemetry" => return,
                    _ => OutputKind::Console,
                };
                let text = body["output"].as_str().unwrap_or_default();
                self.push_output(kind, text);
                self.changed();
            }
            _ => {}
        }
    }

    fn resumed(&mut self) {
        self.generation += 1;
        self.status = Status::Running;
        self.frames.clear();
        self.scopes.clear();
        self.changed();
    }

    fn request_stack(&mut self) {
        let Some(thread) = self.thread_id else {
            return;
        };
        let generation = self.generation;
        self.request(
            "stackTrace",
            json!({ "threadId": thread, "startFrame": 0, "levels": STACK_DEPTH }),
            Pending::StackTrace { generation },
        );
    }

    fn request_scopes(&mut self) {
        let Some(frame) = self.frames.get(self.selected_frame) else {
            return;
        };
        let frame_id = frame.id;
        let generation = self.generation;
        self.request(
            "scopes",
            json!({ "frameId": frame_id }),
            Pending::Scopes {
                generation,
                frame_id,
            },
        );
    }

    fn send_breakpoints(&mut self, path: &Path, lines: &[usize]) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let breakpoints: Vec<Value> = lines.iter().map(|l| json!({ "line": l })).collect();
        self.request(
            "setBreakpoints",
            json!({
                "source": { "path": path.to_string_lossy(), "name": name },
                "breakpoints": breakpoints,
            }),
            Pending::SetBreakpoints,
        );
    }

    // === Output ===

    fn push_output(&mut self, kind: OutputKind, text: &str) {
        let mut text = text.to_string();
        if let Some((partial_kind, partial)) = self.partial.take() {
            if partial_kind == kind {
                text = partial + &text;
            } else {
                self.push_line(partial_kind, partial);
            }
        }
        let mut parts: Vec<&str> = text.split('\n').collect();
        let rest = parts.pop().unwrap_or_default();
        for part in parts {
            self.push_line(kind, part.trim_end_matches('\r').to_string());
        }
        if !rest.is_empty() {
            self.partial = Some((kind, rest.to_string()));
        }
    }

    fn flush_partial(&mut self) {
        if let Some((kind, text)) = self.partial.take() {
            self.push_line(kind, text);
        }
    }

    fn push_line(&mut self, kind: OutputKind, text: String) {
        if self.output.len() >= MAX_OUTPUT_LINES {
            self.output.pop_front();
        }
        self.output.push_back(OutputLine { kind, text });
    }

    // === What the user does ===

    fn control(&mut self, command: &str) {
        if !self.is_stopped() {
            return;
        }
        let thread = self.thread_id.unwrap_or(1);
        self.request(command, json!({ "threadId": thread }), Pending::Control);
        // Not every adapter sends `continued` for a resume it was asked for.
        self.resumed();
    }

    pub fn continue_(&mut self) {
        self.control("continue");
    }

    pub fn step_over(&mut self) {
        self.control("next");
    }

    pub fn step_in(&mut self) {
        self.control("stepIn");
    }

    pub fn step_out(&mut self) {
        self.control("stepOut");
    }

    pub fn pause(&mut self) {
        if self.status != Status::Running {
            return;
        }
        let thread = self.thread_id.unwrap_or(1);
        self.request("pause", json!({ "threadId": thread }), Pending::Control);
    }

    /// End the session, ending the program too.
    pub fn stop(&mut self) {
        if let Some(build) = self.build.take() {
            let _ = build.lock().unwrap_or_else(|e| e.into_inner()).kill();
        }
        if !self.is_active() {
            return;
        }
        self.request(
            "disconnect",
            json!({ "terminateDebuggee": true }),
            Pending::Disconnect,
        );
        self.flush_partial();
        self.status = Status::Ended;
        self.frames.clear();
        self.scopes.clear();
        self.changed();
    }

    /// Show frame `index` of the call stack and its variables.
    pub fn select_frame(&mut self, index: usize) {
        if index >= self.frames.len() || index == self.selected_frame && !self.scopes.is_empty() {
            return;
        }
        self.selected_frame = index;
        self.scopes.clear();
        self.request_scopes();
        self.changed();
    }

    /// Open or close the variable at `path` (indices from the scopes down),
    /// fetching its children the first time.
    pub fn toggle_variable(&mut self, path: &[usize]) {
        let generation = self.generation;
        let Some(node) = node_mut(&mut self.scopes, path) else {
            return;
        };
        if !node.has_children() {
            return;
        }
        if node.expanded {
            node.expanded = false;
        } else if node.children.is_some() {
            node.expanded = true;
        } else {
            let reference = node.reference;
            self.request(
                "variables",
                json!({ "variablesReference": reference }),
                Pending::Variables {
                    generation,
                    path: path.to_vec(),
                },
            );
        }
        self.changed();
    }

    /// Evaluate `expression` in the selected frame; the result goes to the
    /// output.
    pub fn evaluate(&mut self, expression: &str) {
        if !self.is_active() || self.writer.is_none() {
            return;
        }
        self.push_line(OutputKind::Input, expression.to_string());
        let mut arguments = json!({ "expression": expression, "context": "repl" });
        if let Some(frame) = self.frames.get(self.selected_frame) {
            arguments["frameId"] = json!(frame.id);
        }
        self.request("evaluate", arguments, Pending::Evaluate);
        self.changed();
    }

    /// Send the breakpoints of the files whose breakpoints differ from what
    /// the adapter was last told.
    pub fn update_breakpoints(&mut self, all: &[Breakpoint]) {
        let accepted: Vec<Breakpoint> = all
            .iter()
            .filter(|b| self.spec.adapter.accepts(&b.path))
            .cloned()
            .collect();
        self.breakpoints = accepted;
        if !self.configured || !self.is_active() {
            return;
        }
        let files = group_by_file(&self.breakpoints);
        let mut paths: Vec<PathBuf> = files.keys().cloned().collect();
        paths.extend(
            self.sent
                .keys()
                .filter(|p| !files.contains_key(*p))
                .cloned(),
        );
        for path in paths {
            let lines = files.get(&path).cloned().unwrap_or_default();
            if self.sent.get(&path).map(Vec::as_slice).unwrap_or_default() != lines.as_slice() {
                self.send_breakpoints(&path, &lines);
            }
        }
        self.sent = files;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(build) = self.build.take() {
            let _ = build.lock().unwrap_or_else(|e| e.into_inner()).kill();
        }
        if let Some(mut adapter) = self.adapter.take() {
            let _ = adapter.kill();
            let _ = adapter.wait();
        }
    }
}

fn shell_command(line: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", line]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", line]);
        c
    }
}

fn group_by_file(breakpoints: &[Breakpoint]) -> BTreeMap<PathBuf, Vec<usize>> {
    let mut files: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for b in breakpoints {
        files.entry(b.path.clone()).or_default().push(b.line);
    }
    for lines in files.values_mut() {
        lines.sort_unstable();
        lines.dedup();
    }
    files
}

fn parse_frame(frame: &Value) -> Frame {
    Frame {
        id: frame["id"].as_i64().unwrap_or_default(),
        name: frame["name"].as_str().unwrap_or_default().to_string(),
        path: frame["source"]["path"].as_str().map(PathBuf::from),
        line: frame["line"].as_u64().unwrap_or_default() as usize,
    }
}

fn parse_variable(var: &Value) -> Variable {
    Variable {
        name: var["name"].as_str().unwrap_or_default().to_string(),
        value: var["value"].as_str().unwrap_or_default().to_string(),
        type_name: var["type"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string),
        reference: var["variablesReference"].as_i64().unwrap_or_default(),
        children: None,
        expanded: false,
        expensive: false,
    }
}

fn node_mut<'a>(nodes: &'a mut [Variable], path: &[usize]) -> Option<&'a mut Variable> {
    let (first, rest) = path.split_first()?;
    let node = nodes.get_mut(*first)?;
    if rest.is_empty() {
        Some(node)
    } else {
        node_mut(node.children.as_mut()?, rest)
    }
}

#[cfg(test)]
mod tests;
