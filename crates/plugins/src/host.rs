//! Running plugins: one supervised process per plugin per open project.
//!
//! Each plugin has a supervisor thread of its own that starts the process,
//! performs the `initialize` handshake, watches for the process to end and
//! starts it again with backoff, giving up for the session after too many
//! crashes in a row. The UI thread never waits on a plugin: it tells the
//! [`Host`] which plugins a project wants, drains [`HostEvent`]s on its tick,
//! and answers a plugin's requests with [`Host::respond`] when it has the
//! answer.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::protocol::{self, Contributions};
use crate::rpc::{Incoming, Peer, RpcError};

/// What it takes to start one plugin for one project.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub name: String,
    /// The plugin's directory: its working directory.
    pub dir: PathBuf,
    /// The resolved program.
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// The `settings` handed over in `initialize`.
    pub settings: Value,
}

/// A plugin running for a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginKey {
    pub project: PathBuf,
    pub name: String,
}

/// What happened to the plugins, for the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    /// The handshake succeeded; what the plugin contributes.
    Ready {
        key: PluginKey,
        contributions: Contributions,
    },
    /// The plugin replaced what it contributes.
    Contributions {
        key: PluginKey,
        contributions: Contributions,
    },
    /// The process ended or never started. It is started again after
    /// `restart_in`, or, with `None`, not again this session.
    Stopped {
        key: PluginKey,
        reason: String,
        restart_in: Option<Duration>,
    },
    /// A request, owed an answer through [`Host::respond`].
    Request {
        key: PluginKey,
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        key: PluginKey,
        method: String,
        params: Value,
    },
}

impl HostEvent {
    #[must_use]
    pub fn key(&self) -> &PluginKey {
        match self {
            Self::Ready { key, .. }
            | Self::Contributions { key, .. }
            | Self::Stopped { key, .. }
            | Self::Request { key, .. }
            | Self::Notification { key, .. } => key,
        }
    }
}

/// Where a plugin the host holds stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    /// Starting, or waiting to start again after a crash.
    Starting,
    Ready,
    /// Crashed too often: not started again until its launch changes.
    GaveUp,
}

/// A started process: its pipes and a handle to watch and end it.
pub struct Process {
    pub stdout: Box<dyn Read + Send>,
    pub stdin: Box<dyn Write + Send>,
    pub child: Box<dyn ChildProcess>,
}

/// The process side of a [`Process`].
pub trait ChildProcess: Send {
    /// How the process ended, once it has.
    fn try_wait(&mut self) -> Option<String>;
    fn kill(&mut self);
}

/// Starts plugin processes; tests start fakes instead.
pub trait Launcher: Send + Sync {
    /// # Errors
    ///
    /// When the process cannot be started.
    fn launch(&self, launch: &Launch) -> Result<Process, String>;
}

/// Starts real processes, in the plugin's directory, stderr to the log.
pub struct ProcessLauncher;

impl Launcher for ProcessLauncher {
    fn launch(&self, launch: &Launch) -> Result<Process, String> {
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .envs(&launch.env)
            .current_dir(&launch.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group: a signal meant for termide's foreground
        // group does not reach it.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", launch.program.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            let name = launch.name.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::info!("plugin {name}: {line}");
                }
            });
        }
        Ok(Process {
            stdout: Box::new(stdout),
            stdin: Box::new(stdin),
            child: Box::new(child),
        })
    }
}

impl ChildProcess for std::process::Child {
    fn try_wait(&mut self) -> Option<String> {
        match std::process::Child::try_wait(self) {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    fn kill(&mut self) {
        let _ = std::process::Child::kill(self);
        let _ = self.wait();
    }
}

/// How long things may take, and how hard to try again.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub initialize: Duration,
    pub shutdown: Duration,
    pub first_backoff: Duration,
    pub max_backoff: Duration,
    /// This many failures within `crash_window` disable the plugin.
    pub max_crashes: usize,
    pub crash_window: Duration,
    pub poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            initialize: Duration::from_secs(10),
            shutdown: Duration::from_secs(2),
            first_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
            max_crashes: 3,
            crash_window: Duration::from_secs(60),
            poll: Duration::from_millis(100),
        }
    }
}

/// The connection to a running plugin, shared between its supervisor and
/// the host.
struct Live {
    peer: Arc<Peer>,
    /// Set once the handshake succeeded: events are sent only then.
    ready: bool,
    subscriptions: Vec<String>,
}

type LiveSlot = Arc<Mutex<Option<Live>>>;

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An event tagged with the run of the supervisor that sent it, so the
/// events of a replaced run are told from the current one's.
struct Tagged {
    generation: u64,
    event: HostEvent,
}

struct Handle {
    launch: Launch,
    generation: u64,
    stop: Arc<AtomicBool>,
    live: LiveSlot,
    thread: JoinHandle<()>,
}

/// Every plugin running in this instance, over all open projects.
pub struct Host {
    launcher: Arc<dyn Launcher>,
    timing: Timing,
    tx: Sender<Tagged>,
    rx: Receiver<Tagged>,
    plugins: BTreeMap<PluginKey, Handle>,
    next_generation: u64,
}

impl Host {
    #[must_use]
    pub fn new(launcher: Arc<dyn Launcher>, timing: Timing) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            launcher,
            timing,
            tx,
            rx,
            plugins: BTreeMap::new(),
            next_generation: 1,
        }
    }

    /// Make the plugins running for `project` exactly `launches`: start the
    /// new ones, stop the ones no longer wanted, and restart the ones whose
    /// launch changed. One that gave up stays down while its launch is the
    /// same, so syncing again does not wake it.
    pub fn set_project(&mut self, project: &Path, launches: Vec<Launch>) {
        let wanted: HashMap<String, Launch> = launches
            .into_iter()
            .map(|launch| (launch.name.clone(), launch))
            .collect();
        let running: Vec<PluginKey> = self
            .plugins
            .keys()
            .filter(|key| key.project == project)
            .cloned()
            .collect();
        for key in running {
            let keep = wanted
                .get(&key.name)
                .is_some_and(|launch| *launch == self.plugins[&key].launch);
            if !keep {
                self.stop(&key);
            }
        }
        for (name, launch) in wanted {
            let key = PluginKey {
                project: project.to_path_buf(),
                name,
            };
            if !self.plugins.contains_key(&key) {
                self.start(key, launch);
            }
        }
    }

    /// Stop every plugin of `project`.
    pub fn close_project(&mut self, project: &Path) {
        self.set_project(project, Vec::new());
    }

    /// What happened since the last call. Events of plugins stopped or
    /// replaced since are dropped.
    pub fn poll(&mut self) -> Vec<HostEvent> {
        let mut events = Vec::new();
        while let Ok(tagged) = self.rx.try_recv() {
            let current = self
                .plugins
                .get(tagged.event.key())
                .is_some_and(|handle| handle.generation == tagged.generation);
            if current {
                events.push(tagged.event);
            }
        }
        events
    }

    /// Every plugin the host holds, with its state.
    #[must_use]
    pub fn states(&self) -> Vec<(PluginKey, PluginState)> {
        self.plugins
            .iter()
            .map(|(key, handle)| {
                let state = if handle.thread.is_finished() {
                    PluginState::GaveUp
                } else if lock(&handle.live).as_ref().is_some_and(|live| live.ready) {
                    PluginState::Ready
                } else {
                    PluginState::Starting
                };
                (key.clone(), state)
            })
            .collect()
    }

    /// Send the event `method` to every ready plugin of `project` that
    /// subscribed to it.
    pub fn emit(&self, project: &Path, method: &str, params: &Value) {
        for (key, handle) in &self.plugins {
            if key.project != project {
                continue;
            }
            let peer = lock(&handle.live).as_ref().and_then(|live| {
                (live.ready && live.subscriptions.iter().any(|s| s == method))
                    .then(|| Arc::clone(&live.peer))
            });
            if let Some(peer) = peer {
                if let Err(e) = peer.notify(method, params.clone()) {
                    log::debug!("plugin {}: {e}", key.name);
                }
            }
        }
    }

    /// Send the notification `method` to `key`, whatever it subscribed to:
    /// what concerns that plugin alone, such as its own panel.
    pub fn notify(&self, key: &PluginKey, method: &str, params: &Value) {
        if let Some(peer) = self.peer(key) {
            if let Err(e) = peer.notify(method, params.clone()) {
                log::debug!("plugin {}: {e}", key.name);
            }
        }
    }

    /// The connection to `key`, for a request made off the UI thread (a
    /// tool call). `None` while it is not ready.
    #[must_use]
    pub fn peer(&self, key: &PluginKey) -> Option<Arc<Peer>> {
        let handle = self.plugins.get(key)?;
        lock(&handle.live)
            .as_ref()
            .filter(|live| live.ready)
            .map(|live| Arc::clone(&live.peer))
    }

    /// Answer the request `id` that `key` sent.
    pub fn respond(&self, key: &PluginKey, id: &Value, outcome: Result<Value, RpcError>) {
        let Some(handle) = self.plugins.get(key) else {
            return;
        };
        let peer = lock(&handle.live)
            .as_ref()
            .map(|live| Arc::clone(&live.peer));
        if let Some(peer) = peer {
            if let Err(e) = peer.respond(id, outcome) {
                log::debug!("plugin {}: {e}", key.name);
            }
        }
    }

    /// Stop every plugin and wait for them to end, up to the shutdown
    /// timeout and a little more.
    pub fn shutdown(&mut self) {
        let handles: Vec<Handle> = std::mem::take(&mut self.plugins).into_values().collect();
        for handle in &handles {
            handle.stop.store(true, Ordering::Relaxed);
        }
        let deadline = Instant::now() + self.timing.shutdown + Duration::from_secs(1);
        for handle in handles {
            while !handle.thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if handle.thread.is_finished() {
                let _ = handle.thread.join();
            }
        }
    }

    fn start(&mut self, key: PluginKey, launch: Launch) {
        let generation = self.next_generation;
        self.next_generation += 1;
        let stop = Arc::new(AtomicBool::new(false));
        let live: LiveSlot = Arc::new(Mutex::new(None));
        let supervisor = Supervisor {
            key: key.clone(),
            launch: launch.clone(),
            launcher: Arc::clone(&self.launcher),
            timing: self.timing,
            generation,
            tx: self.tx.clone(),
            stop: Arc::clone(&stop),
            live: Arc::clone(&live),
        };
        let thread = std::thread::Builder::new()
            .name(format!("plugin {}", key.name))
            .spawn(move || supervisor.run())
            .expect("spawn a plugin supervisor");
        self.plugins.insert(
            key,
            Handle {
                launch,
                generation,
                stop,
                live,
                thread,
            },
        );
    }

    /// Ask `key`'s supervisor to stop; it finishes on its own thread.
    fn stop(&mut self, key: &PluginKey) {
        if let Some(handle) = self.plugins.remove(key) {
            handle.stop.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        for handle in self.plugins.values() {
            handle.stop.store(true, Ordering::Relaxed);
        }
    }
}

struct Supervisor {
    key: PluginKey,
    launch: Launch,
    launcher: Arc<dyn Launcher>,
    timing: Timing,
    generation: u64,
    tx: Sender<Tagged>,
    stop: Arc<AtomicBool>,
    live: LiveSlot,
}

impl Supervisor {
    fn send(&self, event: HostEvent) {
        let _ = self.tx.send(Tagged {
            generation: self.generation,
            event,
        });
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn run(self) {
        let mut failures: Vec<Instant> = Vec::new();
        let mut backoff = self.timing.first_backoff;
        while !self.stopping() {
            let reason = self.run_once();
            *lock(&self.live) = None;
            if self.stopping() {
                break;
            }
            let now = Instant::now();
            failures.retain(|at| now.duration_since(*at) < self.timing.crash_window);
            failures.push(now);
            let give_up = failures.len() >= self.timing.max_crashes;
            log::warn!("plugin {}: {reason}", self.key.name);
            self.send(HostEvent::Stopped {
                key: self.key.clone(),
                reason,
                restart_in: (!give_up).then_some(backoff),
            });
            if give_up {
                break;
            }
            let until = Instant::now() + backoff;
            while Instant::now() < until && !self.stopping() {
                std::thread::sleep(self.timing.poll);
            }
            backoff = (backoff * 2).min(self.timing.max_backoff);
        }
    }

    /// Start the process and keep it until it ends or the plugin is
    /// stopped; why it ended.
    fn run_once(&self) -> String {
        let mut process = match self.launcher.launch(&self.launch) {
            Ok(process) => process,
            Err(e) => return e,
        };
        let peer = Arc::new(self.connect(process.stdout, process.stdin));
        *lock(&self.live) = Some(Live {
            peer: Arc::clone(&peer),
            ready: false,
            subscriptions: Vec::new(),
        });

        let not_running = || self.stopping();
        match peer.request(
            protocol::INITIALIZE,
            protocol::initialize_params(&self.key, &self.launch),
            self.timing.initialize,
            &not_running,
        ) {
            Ok(result) => {
                let contributions = Contributions::from_value(&result);
                if let Some(live) = lock(&self.live).as_mut() {
                    live.ready = true;
                    live.subscriptions.clone_from(&contributions.subscriptions);
                }
                let _ = peer.notify(protocol::INITIALIZED, json!({}));
                self.send(HostEvent::Ready {
                    key: self.key.clone(),
                    contributions,
                });
            }
            Err(e) if self.stopping() => {
                process.child.kill();
                return e;
            }
            Err(e) => {
                process.child.kill();
                return format!("initialize failed: {e}");
            }
        }

        loop {
            if let Some(status) = process.child.try_wait() {
                return format!("exited: {status}");
            }
            if self.stopping() {
                let _ = peer.request(protocol::SHUTDOWN, json!({}), self.timing.shutdown, &|| {
                    false
                });
                let _ = peer.notify(protocol::EXIT, json!({}));
                let until = Instant::now() + self.timing.shutdown;
                while process.child.try_wait().is_none() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(20));
                }
                process.child.kill();
                return "stopped".to_string();
            }
            std::thread::sleep(self.timing.poll);
        }
    }

    /// The connection, routing what the plugin sends to the host channel.
    fn connect(&self, stdout: Box<dyn Read + Send>, stdin: Box<dyn Write + Send>) -> Peer {
        let key = self.key.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        let live = Arc::clone(&self.live);
        Peer::new(
            stdout,
            stdin,
            move |incoming| {
                let event = match incoming {
                    Incoming::Notification { method, params }
                        if method == protocol::CONTRIBUTIONS_CHANGED =>
                    {
                        let contributions = Contributions::from_value(&params);
                        if let Some(live) = lock(&live).as_mut() {
                            live.subscriptions.clone_from(&contributions.subscriptions);
                        }
                        HostEvent::Contributions {
                            key: key.clone(),
                            contributions,
                        }
                    }
                    Incoming::Notification { method, params } => HostEvent::Notification {
                        key: key.clone(),
                        method,
                        params,
                    },
                    Incoming::Request { id, method, params } => HostEvent::Request {
                        key: key.clone(),
                        id,
                        method,
                        params,
                    },
                };
                let _ = tx.send(Tagged { generation, event });
            },
            || {},
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::pipe;
    use std::sync::atomic::AtomicUsize;

    /// What the fake plugins did, for the tests to look at.
    #[derive(Default)]
    struct Log {
        launches: AtomicUsize,
        received: Mutex<Vec<String>>,
    }

    struct FakeChild {
        exited: Arc<AtomicBool>,
    }

    impl ChildProcess for FakeChild {
        fn try_wait(&mut self) -> Option<String> {
            self.exited
                .load(Ordering::Relaxed)
                .then(|| "exit status: 0".to_string())
        }
        fn kill(&mut self) {
            self.exited.store(true, Ordering::Relaxed);
        }
    }

    /// Plays a plugin on two pipes. `args[0]` picks the part: `good`
    /// answers the handshake, subscribes to `file/saved`, asks termide for a
    /// status item and exits on `exit`; `crash` exits at once.
    struct FakeLauncher {
        log: Arc<Log>,
    }

    impl Launcher for FakeLauncher {
        fn launch(&self, launch: &Launch) -> Result<Process, String> {
            self.log.launches.fetch_add(1, Ordering::Relaxed);
            let (plugin_in, termide_out) = pipe().unwrap();
            let (termide_in, mut plugin_out) = pipe().unwrap();
            let exited = Arc::new(AtomicBool::new(false));
            let part = launch.args.first().cloned().unwrap_or_default();
            let log = Arc::clone(&self.log);
            let done = Arc::clone(&exited);
            std::thread::spawn(move || {
                if part == "crash" {
                    done.store(true, Ordering::Relaxed);
                    return;
                }
                for line in BufReader::new(plugin_in).lines().map_while(Result::ok) {
                    let message: Value = serde_json::from_str(&line).unwrap();
                    let method = message["method"].as_str().unwrap_or("reply").to_string();
                    lock(&log.received).push(method.clone());
                    let reply = |out: &mut std::io::PipeWriter, value: Value| {
                        writeln!(out, "{value}").unwrap();
                    };
                    match method.as_str() {
                        "initialize" => {
                            assert_eq!(message["params"]["plugin"]["name"], "p");
                            reply(
                                &mut plugin_out,
                                json!({
                                    "jsonrpc": "2.0", "id": message["id"],
                                    "result": {
                                        "subscriptions": ["file/saved"],
                                        "commands": [{ "id": "hello", "title": "Hello" }]
                                    }
                                }),
                            );
                        }
                        "initialized" => reply(
                            &mut plugin_out,
                            json!({
                                "jsonrpc": "2.0", "id": "s1", "method": "status/set",
                                "params": { "id": "x", "text": "on" }
                            }),
                        ),
                        "shutdown" => reply(
                            &mut plugin_out,
                            json!({ "jsonrpc": "2.0", "id": message["id"], "result": null }),
                        ),
                        "exit" => break,
                        _ => {}
                    }
                }
                done.store(true, Ordering::Relaxed);
            });
            Ok(Process {
                stdout: Box::new(termide_in),
                stdin: Box::new(termide_out),
                child: Box::new(FakeChild { exited }),
            })
        }
    }

    fn timing() -> Timing {
        Timing {
            initialize: Duration::from_secs(2),
            shutdown: Duration::from_millis(500),
            first_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(20),
            max_crashes: 3,
            crash_window: Duration::from_secs(60),
            poll: Duration::from_millis(5),
        }
    }

    fn launch(part: &str) -> Launch {
        Launch {
            name: "p".into(),
            dir: PathBuf::from("/plugins/p"),
            program: PathBuf::from("/plugins/p/run"),
            args: vec![part.into()],
            env: BTreeMap::new(),
            settings: json!({}),
        }
    }

    fn host() -> (Events, Arc<Log>) {
        let log = Arc::new(Log::default());
        let launcher = Arc::new(FakeLauncher {
            log: Arc::clone(&log),
        });
        let host = Host::new(launcher, timing());
        (
            Events {
                host,
                backlog: Vec::new(),
            },
            log,
        )
    }

    /// The host's events, kept until a test looks for them.
    struct Events {
        host: Host,
        backlog: Vec<HostEvent>,
    }

    impl Events {
        /// The first event `found` picks, polling for up to two seconds;
        /// the events before it are dropped, the ones after it kept.
        fn wait_for<T>(&mut self, mut found: impl FnMut(&HostEvent) -> Option<T>) -> T {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                self.backlog.extend(self.host.poll());
                for (i, event) in self.backlog.iter().enumerate() {
                    if let Some(value) = found(event) {
                        self.backlog.drain(..=i);
                        return value;
                    }
                }
                self.backlog.clear();
                assert!(Instant::now() < deadline, "the event never came");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    fn wait_until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "the condition never held");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_plugin_starts_receives_its_events_and_stops_with_its_project() {
        let (mut events, log) = host();
        let project = PathBuf::from("/projects/a");
        events.host.set_project(&project, vec![launch("good")]);

        let contributions = events.wait_for(|event| match event {
            HostEvent::Ready { contributions, .. } => Some(contributions.clone()),
            _ => None,
        });
        assert_eq!(contributions.commands[0].id, "hello");
        let (key, id) = events.wait_for(|event| match event {
            HostEvent::Request {
                key, id, method, ..
            } if method == "status/set" => Some((key.clone(), id.clone())),
            _ => None,
        });
        events.host.respond(&key, &id, Ok(Value::Null));

        events
            .host
            .emit(&project, "file/opened", &json!({ "path": "/x" }));
        events.host.emit(
            Path::new("/projects/b"),
            "file/saved",
            &json!({ "path": "/y" }),
        );
        events
            .host
            .emit(&project, "file/saved", &json!({ "path": "/x" }));
        wait_until(|| lock(&log.received).contains(&"file/saved".to_string()));
        assert!(!lock(&log.received).contains(&"file/opened".to_string()));
        let saved = lock(&log.received)
            .iter()
            .filter(|m| *m == "file/saved")
            .count();
        assert_eq!(saved, 1, "only its own project's events reach it");

        events.host.close_project(&project);
        wait_until(|| lock(&log.received).contains(&"exit".to_string()));
        assert!(events.host.states().is_empty());
        assert_eq!(log.launches.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_crashing_plugin_is_restarted_then_given_up_on() {
        let (mut events, log) = host();
        let project = PathBuf::from("/projects/a");
        events.host.set_project(&project, vec![launch("crash")]);

        let mut restarts = Vec::new();
        events.wait_for(|event| match event {
            HostEvent::Stopped { restart_in, .. } => {
                restarts.push(*restart_in);
                restart_in.is_none().then_some(())
            }
            _ => None,
        });
        assert_eq!(restarts.len(), 3);
        assert!(restarts[..2].iter().all(Option::is_some));
        wait_until(|| events.host.states()[0].1 == PluginState::GaveUp);
        assert_eq!(log.launches.load(Ordering::Relaxed), 3);

        // Syncing the same launch does not wake it; a changed one does.
        events.host.set_project(&project, vec![launch("crash")]);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(log.launches.load(Ordering::Relaxed), 3);
        events.host.set_project(&project, vec![launch("good")]);
        events.wait_for(|event| matches!(event, HostEvent::Ready { .. }).then_some(()));
        events.host.shutdown();
        assert!(lock(&log.received).contains(&"shutdown".to_string()));
    }
}
