//! Language Server Protocol integration for termide.
//!
//! This crate provides LSP client functionality for code intelligence features:
//! - Code completion
//! - Go to definition
//! - Hover information
//! - Diagnostics (errors/warnings)
//!
//! # Architecture
//!
//! The LSP integration uses a threading model consistent with the rest of termide:
//! - `std::thread` for background operations
//! - `std::sync::mpsc` channels for async communication
//! - Non-blocking `try_recv()` polling in the main event loop

pub mod extra;
mod protocol;
mod server;
mod workspace;

pub use lsp_types::CompletionTriggerKind;
pub use protocol::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
pub use server::{CrashNotice, LspServer, LspServerConfig, ServerStatus};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lsp_types::{
    CompletionResponse, GotoDefinitionResponse, Hover, Location, Position,
    PublishDiagnosticsParams, TextDocumentContentChangeEvent, Uri, WorkspaceEdit,
};
use url::Url;

/// Convert a file path to an LSP Uri. Used across the crate (lib, server).
/// The `file://` URI of `path`, as the servers are told it.
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let url = Url::from_file_path(path).ok()?;
    url.as_str().parse().ok()
}

/// Configuration for LSP servers
#[derive(Debug, Clone, Default)]
pub struct LspConfig {
    /// Per-language server configurations
    pub servers: HashMap<String, LspServerConfig>,
}

/// A server's identity: its language and the workspace it runs in.
type ServerKey = (String, PathBuf);

/// How often a crashed server is restarted on its own: at most
/// `max_restarts` within `window`, the n-th after `base_backoff * 2^n`.
/// Past that it stays crashed until restarted by hand.
#[derive(Debug, Clone, Copy)]
struct RestartPolicy {
    max_restarts: usize,
    window: Duration,
    base_backoff: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            window: Duration::from_secs(180),
            base_backoff: Duration::from_millis(500),
        }
    }
}

/// The server process behind one key, through its restarts.
struct ServerSlot {
    state: SlotState,
    /// Id of the live server, or of the one being started. Every process
    /// gets a new one, so it doubles as the generation editors sync against.
    id: u64,
    /// When the automatic restarts within the policy window were made.
    restarts: Vec<Instant>,
    /// Whether a process of this slot ever went live. A slot whose first
    /// start failed (say, the binary is missing) is kept, so that a manual
    /// restart can start it, but it is never retried on its own.
    started: bool,
}

impl ServerSlot {
    /// No process ever ran: the first start failed, and so did any manual
    /// retry since. Such a slot is not a server for anything but a restart.
    fn never_started(&self) -> bool {
        !self.started && matches!(self.state, SlotState::Failed)
    }
}

enum SlotState {
    Live(LspServer),
    /// The old process is shut down and a new one started, off the UI
    /// thread; the result arrives through `restarted_rx`.
    Restarting,
    /// The last restart could not start the server.
    Failed,
}

/// A restart finished in the background.
struct Restarted {
    key: ServerKey,
    id: u64,
    result: Result<LspServer>,
}

/// Manages multiple LSP servers (one per language/workspace)
pub struct LspManager {
    /// Servers keyed by (language_id, workspace_root)
    servers: HashMap<ServerKey, ServerSlot>,
    /// Configuration
    config: LspConfig,
    /// Diagnostics receiver for all servers
    diagnostics_rx: mpsc::Receiver<PublishDiagnosticsParams>,
    /// Sender for diagnostics (cloned to each server)
    diagnostics_tx: mpsc::Sender<PublishDiagnosticsParams>,
    /// Receiver for server-initiated `workspace/applyEdit` edits
    apply_edit_rx: mpsc::Receiver<WorkspaceEdit>,
    /// Sender for apply-edit requests (cloned to each server)
    apply_edit_tx: mpsc::Sender<WorkspaceEdit>,
    /// Workspace roots the toolchain named, keyed by (language_id, package
    /// root), so it is asked once per package rather than on every request.
    workspace_roots: Mutex<HashMap<(String, PathBuf), PathBuf>>,
    /// The workspace of the project file last opened per language, whose
    /// server takes the library files opened from it (see
    /// [`workspace::is_library_file`]).
    last_roots: Mutex<HashMap<String, PathBuf>>,
    /// Ids of servers whose connection was lost without a shutdown.
    crash_tx: mpsc::Sender<u64>,
    crash_rx: mpsc::Receiver<u64>,
    /// Crash notices of servers not yet handed over.
    crashes: Vec<u64>,
    /// Servers started by restart threads.
    restarted_tx: mpsc::Sender<Restarted>,
    restarted_rx: mpsc::Receiver<Restarted>,
    /// Set by [`Self::shutdown`]; a restart thread that finds it set shuts
    /// its server down instead of handing it over. Checked and sent under
    /// this lock, so no server is left in the channel after the drain.
    closed: Arc<Mutex<bool>>,
    next_id: u64,
    restart_policy: RestartPolicy,
}

impl std::fmt::Debug for LspManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspManager")
            .field("servers_count", &self.servers.len())
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl LspManager {
    /// Create a new LSP manager with the given configuration
    pub fn new(config: LspConfig) -> Self {
        let (diagnostics_tx, diagnostics_rx) = mpsc::channel();
        let (apply_edit_tx, apply_edit_rx) = mpsc::channel();
        let (crash_tx, crash_rx) = mpsc::channel();
        let (restarted_tx, restarted_rx) = mpsc::channel();
        Self {
            servers: HashMap::new(),
            config,
            diagnostics_rx,
            diagnostics_tx,
            apply_edit_rx,
            apply_edit_tx,
            workspace_roots: Mutex::new(HashMap::new()),
            last_roots: Mutex::new(HashMap::new()),
            crash_tx,
            crash_rx,
            crashes: Vec::new(),
            restarted_tx,
            restarted_rx,
            closed: Arc::new(Mutex::new(false)),
            next_id: 1,
            restart_policy: RestartPolicy::default(),
        }
    }

    /// Detect language from file name or extension
    pub fn detect_language(path: &Path) -> Option<String> {
        let name = path.file_name()?.to_str()?;
        // Dockerfile and Compose files carry no distinctive extension, so
        // they are recognized by name, case-insensitively: `DOCKERFILE` and
        // `COMPOSE.yaml` are ordinary names on a case-insensitive filesystem,
        // and lowercase `dockerfile` is common on Linux. Every pattern here is
        // lowercase, so comparing against the lowered name is enough.
        let lower = name.to_lowercase();
        if lower == "dockerfile"
            || lower.starts_with("dockerfile.")
            || lower.ends_with(".dockerfile")
        {
            return Some("dockerfile".to_string());
        }
        static COMPOSE_PATTERNS: OnceLock<Vec<glob::Pattern>> = OnceLock::new();
        let compose_patterns = COMPOSE_PATTERNS.get_or_init(|| {
            [
                "compose.yaml",
                "compose.yml",
                "compose.*.yaml",
                "compose.*.yml",
                "docker-compose.yaml",
                "docker-compose.yml",
                "docker-compose.*.yaml",
                "docker-compose.*.yml",
            ]
            .iter()
            .filter_map(|pattern| glob::Pattern::new(pattern).ok())
            .collect()
        });
        if compose_patterns.iter().any(|p| p.matches(&lower)) {
            return Some("dockercompose".to_string());
        }

        let ext = path.extension()?.to_str()?;
        let lang = match ext {
            "rs" => "rust",
            "py" => "python",
            "js" => "javascript",
            "ts" => "typescript",
            "tsx" => "typescriptreact",
            "jsx" => "javascriptreact",
            "go" => "go",
            "c" | "h" => "c",
            "cpp" | "cc" | "cxx" | "hpp" => "cpp",
            "java" => "java",
            "rb" => "ruby",
            "php" => "php",
            "hs" => "haskell",
            "nix" => "nix",
            "html" => "html",
            "css" => "css",
            "json" => "json",
            "toml" => "toml",
            "yaml" | "yml" => "yaml",
            "sh" | "bash" => "shellscript",
            "md" => "markdown",
            "tf" => "terraform",
            "tfvars" => "terraform-vars",
            _ => return None,
        };
        Some(lang.to_string())
    }

    /// Find workspace root based on root markers
    pub fn find_workspace_root(path: &Path, markers: &[String]) -> Option<PathBuf> {
        let mut current = if path.is_file() { path.parent()? } else { path };

        loop {
            for marker in markers {
                if current.join(marker).exists() {
                    return Some(current.to_path_buf());
                }
            }

            match current.parent() {
                Some(parent) => current = parent,
                None => return None,
            }
        }
    }

    /// The root a server for `file_path` runs in: the nearest root marker,
    /// widened to the workspace the toolchain puts that package in (see
    /// [`workspace`]). Files without a marker use their own directory.
    fn resolve_root(&self, lang: &str, file_path: &Path, markers: &[String]) -> PathBuf {
        if workspace::is_library_file(lang, file_path) {
            if let Some(root) = self.library_root(lang) {
                return root;
            }
        }
        let Some(package_root) = Self::find_workspace_root(file_path, markers) else {
            return file_path.parent().unwrap_or(Path::new("/")).to_path_buf();
        };
        let key = (lang.to_string(), package_root);
        let mut cache = self
            .workspace_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(root) = cache.get(&key) {
            return root.clone();
        }
        let root = workspace::workspace_root(lang, &key.1).unwrap_or_else(|| key.1.clone());
        cache.insert(key, root.clone());
        root
    }

    /// The running server's workspace a library file of `lang` goes to: the
    /// one of the project file opened last, or else any running for `lang`.
    /// `None` when none runs, and the file gets a server of its own.
    fn library_root(&self, lang: &str) -> Option<PathBuf> {
        let last = self
            .last_roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(lang)
            .cloned();
        let runs = |key: &ServerKey| {
            self.servers
                .get(key)
                .is_some_and(|slot| !slot.never_started())
        };
        last.filter(|root| runs(&(lang.to_string(), root.clone())))
            .or_else(|| {
                self.servers
                    .keys()
                    .filter(|key| key.0 == lang && runs(key))
                    .map(|(_, root)| root.clone())
                    .min()
            })
    }

    /// Ensure a server is running for the given language and file
    pub fn ensure_server(&mut self, lang: &str, file_path: &Path) -> Result<()> {
        let server_config = self
            .config
            .servers
            .get(lang)
            .context(format!("No LSP server configured for language: {}", lang))?
            .clone();

        let workspace_root = self.resolve_root(lang, file_path, &server_config.root_markers);

        let key = (lang.to_string(), workspace_root.clone());
        if !workspace::is_library_file(lang, file_path) {
            self.last_roots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(lang.to_string(), workspace_root.clone());
        }

        // A server whose start failed is tried again when another of its
        // files is opened, as before it had a slot; any other slot stands.
        if self
            .servers
            .get(&key)
            .is_some_and(|slot| !slot.never_started())
        {
            return Ok(());
        }

        log::info!("Starting LSP server for {} in {:?}", lang, workspace_root);

        let id = self.next_id();
        let started = LspServer::start(
            lang.to_string(),
            server_config,
            workspace_root,
            self.diagnostics_tx.clone(),
            self.apply_edit_tx.clone(),
            CrashNotice::new(self.crash_tx.clone(), id),
        );
        let (state, result) = match started {
            Ok(server) => (SlotState::Live(server), Ok(())),
            // Remembered, so "Restart language server" can start it later.
            Err(e) => (SlotState::Failed, Err(e)),
        };
        let started = matches!(state, SlotState::Live(_));
        self.servers.insert(
            key,
            ServerSlot {
                state,
                id,
                restarts: Vec::new(),
                started,
            },
        );
        result
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The key of the server that handles this file.
    fn server_key(&self, lang: &str, file_path: &Path) -> Option<ServerKey> {
        let server_config = self.config.servers.get(lang)?;
        let workspace_root = self.resolve_root(lang, file_path, &server_config.root_markers);
        Some((lang.to_string(), workspace_root))
    }

    /// The slot of the server handling this file, unless none ever started.
    fn get_slot(&self, lang: &str, file_path: &Path) -> Option<&ServerSlot> {
        self.servers
            .get(&self.server_key(lang, file_path)?)
            .filter(|slot| !slot.never_started())
    }

    /// Whether this file's server could not be started, and so far was not
    /// started by hand either: [`Self::restart_server`] starts it.
    pub fn server_start_failed(&self, lang: &str, file_path: &Path) -> bool {
        self.server_key(lang, file_path)
            .and_then(|key| self.servers.get(&key))
            .is_some_and(ServerSlot::never_started)
    }

    /// Get server for language and file (if running)
    fn get_server(&self, lang: &str, file_path: &Path) -> Option<&LspServer> {
        match &self.get_slot(lang, file_path)?.state {
            SlotState::Live(server) => Some(server),
            SlotState::Restarting | SlotState::Failed => None,
        }
    }

    /// The generation of the server handling this file: it changes with
    /// every new process, and an editor that opened its document under
    /// another one sends `didOpen` again (see [`Self::poll_restarts`]).
    /// `None` while no server process is live.
    pub fn server_generation(&self, lang: &str, file_path: &Path) -> Option<u64> {
        let slot = self.get_slot(lang, file_path)?;
        matches!(slot.state, SlotState::Live(_)).then_some(slot.id)
    }

    /// Act on crashed servers and finished restarts. Non-blocking: a crash
    /// schedules a restart within the budget, and the restart runs on a
    /// thread of its own. `true` when a new server went live, after which
    /// every open document must be opened again in it: editors compare
    /// [`Self::server_generation`] with the one they opened under.
    pub fn poll_restarts(&mut self) -> bool {
        let mut went_live = false;
        while let Ok(restarted) = self.restarted_rx.try_recv() {
            let Some(slot) = self.servers.get_mut(&restarted.key) else {
                continue;
            };
            if slot.id != restarted.id || !matches!(slot.state, SlotState::Restarting) {
                continue;
            }
            match restarted.result {
                Ok(server) => {
                    log::info!("LSP server for {:?} restarted", restarted.key);
                    slot.state = SlotState::Live(server);
                    slot.started = true;
                    went_live = true;
                }
                // A server that never started is only started by hand: a
                // missing binary is not looked for again and again.
                Err(e) if !slot.started => {
                    log::error!("LSP server for {:?} failed to start: {e:#}", restarted.key);
                    slot.state = SlotState::Failed;
                }
                Err(e) => {
                    log::error!(
                        "LSP server for {:?} failed to restart: {e:#}",
                        restarted.key
                    );
                    slot.state = SlotState::Failed;
                    self.restart_automatically(restarted.key);
                }
            }
        }

        // A new server can crash before it is handed over: its notice then
        // waits for it to go live rather than being lost.
        self.crashes.extend(self.crash_rx.try_iter());
        for id in std::mem::take(&mut self.crashes) {
            let Some((key, slot)) = self.servers.iter().find(|(_, slot)| slot.id == id) else {
                continue;
            };
            match slot.state {
                SlotState::Live(_) => self.restart_automatically(key.clone()),
                SlotState::Restarting => self.crashes.push(id),
                SlotState::Failed => {}
            }
        }
        went_live
    }

    /// Restart a crashed server unless it spent its budget.
    fn restart_automatically(&mut self, key: ServerKey) {
        let policy = self.restart_policy;
        let Some(slot) = self.servers.get_mut(&key) else {
            return;
        };
        let now = Instant::now();
        slot.restarts
            .retain(|at| now.duration_since(*at) < policy.window);
        let attempt = slot.restarts.len();
        if attempt >= policy.max_restarts {
            log::error!(
                "LSP server for {key:?} crashed {attempt} times within {:?}; not restarting it again",
                policy.window
            );
            return;
        }
        slot.restarts.push(now);
        let backoff = policy.base_backoff * 2u32.saturating_pow(attempt as u32);
        log::warn!(
            "Restarting LSP server for {key:?} in {backoff:?} (restart {} of {})",
            attempt + 1,
            policy.max_restarts
        );
        self.spawn_restart(key, backoff);
    }

    /// Restart the server handling this file by hand, whatever its state,
    /// with a fresh budget of automatic restarts. A server whose start failed
    /// is started; once live, editors open their documents in it as after
    /// any restart. `false` when no server was ever tried for the file.
    pub fn restart_server(&mut self, lang: &str, file_path: &Path) -> bool {
        let Some(key) = self.server_key(lang, file_path) else {
            return false;
        };
        let Some(slot) = self.servers.get_mut(&key) else {
            return false;
        };
        slot.restarts.clear();
        if !matches!(slot.state, SlotState::Restarting) {
            log::info!("Restarting LSP server for {key:?} on request");
            self.spawn_restart(key, Duration::ZERO);
        }
        true
    }

    /// Replace the slot's process off the UI thread: shut the old one down,
    /// clear the diagnostics it left, wait `backoff`, start a new one.
    fn spawn_restart(&mut self, key: ServerKey, backoff: Duration) {
        let Some(config) = self.config.servers.get(&key.0).cloned() else {
            return;
        };
        let id = self.next_id();
        let Some(slot) = self.servers.get_mut(&key) else {
            return;
        };
        let old = match std::mem::replace(&mut slot.state, SlotState::Restarting) {
            SlotState::Live(server) => Some(server),
            SlotState::Restarting | SlotState::Failed => None,
        };
        slot.id = id;

        let diagnostics_tx = self.diagnostics_tx.clone();
        let apply_edit_tx = self.apply_edit_tx.clone();
        let notice = CrashNotice::new(self.crash_tx.clone(), id);
        let restarted_tx = self.restarted_tx.clone();
        let closed = self.closed.clone();
        std::thread::spawn(move || {
            if let Some(old) = old {
                for uri in old.shutdown() {
                    let _ = diagnostics_tx.send(PublishDiagnosticsParams {
                        uri,
                        diagnostics: Vec::new(),
                        version: None,
                    });
                }
            }
            std::thread::sleep(backoff);
            let is_closed = || *closed.lock().unwrap_or_else(|e| e.into_inner());
            if is_closed() {
                return;
            }
            let (lang, root) = key.clone();
            let result =
                LspServer::start(lang, config, root, diagnostics_tx, apply_edit_tx, notice);
            let closed = closed.lock().unwrap_or_else(|e| e.into_inner());
            let result = if *closed {
                result
            } else {
                match restarted_tx.send(Restarted { key, id, result }) {
                    Ok(()) => return,
                    Err(mpsc::SendError(restarted)) => restarted.result,
                }
            };
            drop(closed);
            if let Ok(server) = result {
                server.shutdown();
            }
        });
    }

    /// Request completion at position
    pub fn completion(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
        trigger_kind: CompletionTriggerKind,
        trigger_character: Option<String>,
    ) -> Option<mpsc::Receiver<Option<CompletionResponse>>> {
        let from_server = self.get_server(lang, file_path).and_then(|server| {
            let uri = path_to_uri(file_path)?;
            Some(server.completion(uri, position, trigger_kind, trigger_character))
        });
        let extras = extra::extra()
            .map(|extra| extra.completion(lang, file_path, position))
            .unwrap_or_default();
        extra::merged(from_server, extras, extra::merge_completions)
    }

    /// Completion trigger characters advertised by the server handling this
    /// file, or an empty list if no server is running for it yet.
    pub fn completion_trigger_characters(&self, lang: &str, file_path: &Path) -> Vec<String> {
        self.get_server(lang, file_path)
            .map(|server| server.completion_trigger_characters())
            .unwrap_or_default()
    }

    /// Request code actions for a range in this file.
    pub fn code_action(
        &self,
        lang: &str,
        file_path: &Path,
        range: lsp_types::Range,
        diagnostics: Vec<lsp_types::Diagnostic>,
    ) -> Option<mpsc::Receiver<Option<lsp_types::CodeActionResponse>>> {
        let extras = extra::extra()
            .map(|extra| extra.code_action(lang, file_path, range, &diagnostics))
            .unwrap_or_default();
        let from_server = self.get_server(lang, file_path).and_then(|server| {
            let uri = path_to_uri(file_path)?;
            Some(server.code_action(uri, range, diagnostics))
        });
        extra::merged(from_server, extras, extra::merge_actions)
    }

    /// Whether the server for this file resolves code actions lazily.
    pub fn supports_code_action_resolve(&self, lang: &str, file_path: &Path) -> bool {
        self.get_server(lang, file_path)
            .map(|server| server.supports_code_action_resolve())
            .unwrap_or(false)
    }

    /// Resolve a code action (fill in its `edit`) via `codeAction/resolve`.
    pub fn code_action_resolve(
        &self,
        lang: &str,
        file_path: &Path,
        action: lsp_types::CodeAction,
    ) -> Option<mpsc::Receiver<Option<lsp_types::CodeAction>>> {
        let server = self.get_server(lang, file_path)?;
        Some(server.code_action_resolve(action))
    }

    /// Execute a server command (`workspace/executeCommand`) for a command-based
    /// code action. The resulting edit (if any) arrives via `poll_apply_edit`.
    pub fn execute_command(
        &self,
        lang: &str,
        file_path: &Path,
        command: String,
        arguments: Vec<serde_json::Value>,
    ) {
        if let Some(server) = self.get_server(lang, file_path) {
            server.execute_command(command, arguments);
        }
    }

    /// Poll for a server-initiated `workspace/applyEdit` edit (non-blocking).
    pub fn poll_apply_edit(&self) -> Option<WorkspaceEdit> {
        self.apply_edit_rx.try_recv().ok()
    }

    /// Request hover info at position
    pub fn hover(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Option<mpsc::Receiver<Option<Hover>>> {
        let from_server = self.get_server(lang, file_path).and_then(|server| {
            let uri = path_to_uri(file_path)?;
            Some(server.hover(uri, position))
        });
        let extras = extra::extra()
            .map(|extra| extra.hover(lang, file_path, position))
            .unwrap_or_default();
        extra::merged(from_server, extras, extra::merge_hovers)
    }

    /// Request go-to-definition at position
    pub fn goto_definition(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Option<mpsc::Receiver<Option<GotoDefinitionResponse>>> {
        let from_server = self.get_server(lang, file_path).and_then(|server| {
            let uri = path_to_uri(file_path)?;
            Some(server.goto_definition(uri, position))
        });
        let extras = extra::extra()
            .map(|extra| extra.definition(lang, file_path, position))
            .unwrap_or_default();
        extra::merged(from_server, extras, extra::merge_definitions)
    }

    /// Request find-references at position
    pub fn references(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
        include_declaration: bool,
    ) -> Option<mpsc::Receiver<Option<Vec<Location>>>> {
        let from_server = self.get_server(lang, file_path).and_then(|server| {
            let uri = path_to_uri(file_path)?;
            Some(server.references(uri, position, include_declaration))
        });
        let extras = extra::extra()
            .map(|extra| extra.references(lang, file_path, position))
            .unwrap_or_default();
        extra::merged(from_server, extras, extra::merge_references)
    }

    /// Request rename symbol at position
    pub fn rename(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
        new_name: String,
    ) -> Option<mpsc::Receiver<Option<WorkspaceEdit>>> {
        let server = self.get_server(lang, file_path)?;
        let uri = path_to_uri(file_path)?;
        Some(server.rename(uri, position, new_name))
    }

    /// Send didOpen notification
    pub fn did_open(&self, lang: &str, file_path: &Path, version: i32, text: &str) {
        if let Some(server) = self.get_server(lang, file_path) {
            if let Some(uri) = path_to_uri(file_path) {
                server.did_open(uri, lang.to_string(), version, text.to_string());
            }
        }
    }

    /// Whether the server handling this file accepts ranged `didChange`
    /// notifications. `false` for a server that advertises `Full`, and for one
    /// that has not reported its capabilities yet.
    pub fn supports_incremental_sync(&self, lang: &str, file_path: &Path) -> bool {
        self.get_server(lang, file_path)
            .map(|server| server.supports_incremental_sync())
            .unwrap_or(false)
    }

    /// Send didChange notification carrying the whole document.
    pub fn did_change_full(&self, lang: &str, file_path: &Path, version: i32, text: &str) {
        if let Some(server) = self.get_server(lang, file_path) {
            if let Some(uri) = path_to_uri(file_path) {
                server.did_change_full(uri, version, text.to_string());
            }
        }
    }

    /// Send didChange notification carrying ranged changes.
    ///
    /// The caller is responsible for checking
    /// [`Self::supports_incremental_sync`] first.
    pub fn did_change_incremental(
        &self,
        lang: &str,
        file_path: &Path,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) {
        if let Some(server) = self.get_server(lang, file_path) {
            if let Some(uri) = path_to_uri(file_path) {
                server.did_change(uri, version, changes);
            }
        }
    }

    /// Send didClose notification
    pub fn did_close(&self, lang: &str, file_path: &Path) {
        if let Some(server) = self.get_server(lang, file_path) {
            if let Some(uri) = path_to_uri(file_path) {
                server.did_close(uri);
            }
        }
    }

    /// Send didSave notification
    ///
    /// This triggers full project analysis in rust-analyzer and other LSP servers,
    /// which is necessary for detecting logical errors like unresolved modules.
    pub fn did_save(&self, lang: &str, file_path: &Path, text: Option<&str>) {
        if let Some(server) = self.get_server(lang, file_path) {
            if let Some(uri) = path_to_uri(file_path) {
                server.did_save(uri, text.map(String::from));
            }
        }
    }

    /// Poll for diagnostics updates (non-blocking)
    pub fn poll_diagnostics(&self) -> Option<PublishDiagnosticsParams> {
        self.diagnostics_rx.try_recv().ok()
    }

    /// Get server status for a language and file path
    pub fn server_status(&self, lang: &str, file_path: &Path) -> Option<ServerStatus> {
        Some(match &self.get_slot(lang, file_path)?.state {
            SlotState::Live(server) => server.status(),
            SlotState::Restarting => ServerStatus::Restarting,
            SlotState::Failed => ServerStatus::Crashed,
        })
    }

    /// Check if server is effectively ready (Running, or Indexing with no active progress)
    pub fn server_is_ready(&self, lang: &str, file_path: &Path) -> bool {
        self.get_server(lang, file_path)
            .map(|s| s.is_ready())
            .unwrap_or(false)
    }

    /// Check if server is actively indexing (has active progress tokens)
    pub fn server_is_indexing(&self, lang: &str, file_path: &Path) -> bool {
        self.get_server(lang, file_path)
            .map(|s| s.is_indexing())
            .unwrap_or(false)
    }

    /// Shut down all servers side by side, so the bounded wait of each
    /// (see [`LspServer::shutdown`]) is paid once, not once per server.
    /// Restarts still in flight shut their own server down.
    pub fn shutdown(&mut self) {
        // Under the lock restart threads hand servers over with, so every
        // server handed over is in the channel by the time it is drained.
        let restarted: Vec<_> = {
            let mut closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
            *closed = true;
            self.restarted_rx.try_iter().collect()
        };
        let live = self
            .servers
            .drain()
            .filter_map(|(key, slot)| match slot.state {
                SlotState::Live(server) => Some((key, server)),
                SlotState::Restarting | SlotState::Failed => None,
            });
        let restarted = restarted
            .into_iter()
            .filter_map(|r| r.result.ok().map(|server| (r.key, server)));
        let handles: Vec<_> = live
            .chain(restarted)
            .map(|(key, server)| {
                std::thread::spawn(move || {
                    log::info!("Shutting down LSP server for {:?}", key);
                    server.shutdown();
                })
            })
            .collect();
        for handle in handles {
            let _ = handle.join();
        }
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language() {
        assert_eq!(
            LspManager::detect_language(Path::new("main.rs")),
            Some("rust".to_string())
        );
        assert_eq!(
            LspManager::detect_language(Path::new("script.py")),
            Some("python".to_string())
        );
        assert_eq!(
            LspManager::detect_language(Path::new("app.tsx")),
            Some("typescriptreact".to_string())
        );
        assert_eq!(LspManager::detect_language(Path::new("unknown.xyz")), None);

        let cases = [
            ("index.php", "php"),
            ("main.tf", "terraform"),
            ("prod.tfvars", "terraform-vars"),
            ("Dockerfile", "dockerfile"),
            ("Dockerfile.dev", "dockerfile"),
            ("app.Dockerfile", "dockerfile"),
            ("docker/Dockerfile", "dockerfile"),
            ("compose.yaml", "dockercompose"),
            ("compose.yml", "dockercompose"),
            ("docker-compose.yaml", "dockercompose"),
            ("docker-compose.yml", "dockercompose"),
            ("compose.override.yaml", "dockercompose"),
            ("docker-compose.override.yml", "dockercompose"),
            ("docker-compose.prod.yaml", "dockercompose"),
            ("composer.yaml", "yaml"),
            ("docker-composer.yml", "yaml"),
            ("config.yaml", "yaml"),
            (".github/workflows/ci.yml", "yaml"),
            // Case variants: the name match is case-insensitive, so these
            // resolve the same way on a case-insensitive filesystem and on a
            // case-sensitive one.
            ("DOCKERFILE", "dockerfile"),
            ("App.Dockerfile", "dockerfile"),
            ("Compose.yaml", "dockercompose"),
            ("DOCKER-COMPOSE.PROD.yml", "dockercompose"),
            ("composer.yml", "yaml"),
        ];
        for (path, lang) in cases {
            assert_eq!(
                LspManager::detect_language(Path::new(path)),
                Some(lang.to_string()),
                "{path}"
            );
        }
    }

    #[test]
    fn test_find_workspace_root() {
        // This test depends on the actual filesystem
        // In a real test, we'd use tempfile to create a test directory structure
    }

    /// Restarts against stand-in servers written in `sh`.
    #[cfg(unix)]
    mod restart {
        use super::*;

        /// A manager running `script` for language `test` in `dir`, with
        /// the default budget but no time to wait between restarts.
        fn manager(dir: &Path, script: &str) -> (LspManager, PathBuf) {
            let mut config = LspConfig::default();
            config.servers.insert(
                "test".into(),
                LspServerConfig {
                    command: "sh".into(),
                    args: vec!["-c".into(), script.into()],
                    root_markers: Vec::new(),
                },
            );
            let mut manager = LspManager::new(config);
            manager.restart_policy.base_backoff = Duration::from_millis(10);
            let file = dir.join("a.rs");
            std::fs::write(&file, "").unwrap();
            manager.ensure_server("test", &file).unwrap();
            (manager, file)
        }

        /// Poll until `done`, failing after a few seconds.
        fn poll_until(manager: &mut LspManager, mut done: impl FnMut(&mut LspManager) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done(manager) {
                assert!(Instant::now() < deadline, "timed out");
                manager.poll_restarts();
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// How many times the server process was started.
        fn starts(dir: &Path) -> usize {
            std::fs::read_to_string(dir.join("starts"))
                .map(|s| s.lines().count())
                .unwrap_or(0)
        }

        /// Waits for `expected` starts and a crash, then keeps polling for a
        /// while and checks the server is not started again.
        fn assert_stays_crashed(
            manager: &mut LspManager,
            dir: &Path,
            file: &Path,
            expected: usize,
        ) {
            poll_until(manager, |m| {
                starts(dir) >= expected
                    && m.server_status("test", file) == Some(ServerStatus::Crashed)
            });
            for _ in 0..30 {
                manager.poll_restarts();
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(starts(dir), expected, "restarted past its budget");
            assert_eq!(
                manager.server_status("test", file),
                Some(ServerStatus::Crashed)
            );
        }

        /// A server that crashes is started again, under a new generation,
        /// and the diagnostics it published are cleared before the new one
        /// can publish any.
        #[test]
        fn a_crashed_server_is_restarted_and_its_diagnostics_cleared() {
            let dir = tempfile::tempdir().unwrap();
            let uri = path_to_uri(&dir.path().join("a.rs")).unwrap();
            let publish = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": uri.as_str(),
                    "diagnostics": [{
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 1}
                        },
                        "message": "bad"
                    }]
                }
            })
            .to_string();
            // The first run publishes and dies; the second stays up.
            let script = format!(
                "echo >> starts; if [ -e crashed ]; then exec sleep 60; fi; : > crashed; \
                 printf 'Content-Length: {}\\r\\n\\r\\n%s' '{publish}'",
                publish.len()
            );
            let (mut manager, file) = manager(dir.path(), &script);
            let first = manager.server_generation("test", &file);
            assert!(first.is_some());

            poll_until(&mut manager, |m| {
                m.server_generation("test", &file)
                    .is_some_and(|g| Some(g) != first)
            });
            assert_eq!(starts(dir.path()), 2);
            assert_ne!(
                manager.server_status("test", &file),
                Some(ServerStatus::Crashed)
            );

            let published: Vec<_> = std::iter::from_fn(|| manager.poll_diagnostics()).collect();
            let counts: Vec<_> = published.iter().map(|p| p.diagnostics.len()).collect();
            assert_eq!(counts, [1, 0], "published, then cleared");
            assert!(published.iter().all(|p| p.uri == uri));
        }

        /// A server that keeps crashing is restarted as often as the budget
        /// allows, then left crashed; restarting it by hand starts it again
        /// with a fresh budget.
        #[test]
        fn a_crash_loop_stops_at_the_budget_and_a_manual_restart_resets_it() {
            let dir = tempfile::tempdir().unwrap();
            let (mut manager, file) = manager(dir.path(), "echo >> starts");
            let budget = manager.restart_policy.max_restarts;

            assert_stays_crashed(&mut manager, dir.path(), &file, 1 + budget);

            assert!(manager.restart_server("test", &file));
            assert_ne!(
                manager.server_status("test", &file),
                Some(ServerStatus::Crashed),
                "restarting shows at once"
            );
            assert_stays_crashed(&mut manager, dir.path(), &file, 2 + 2 * budget);
        }

        /// Writes an executable `sh` script at `path`: a server installed
        /// after its first start failed. Written aside and renamed in, so no
        /// thread forking meanwhile holds it open for writing (`ETXTBSY`).
        fn install(path: &Path, script: &str) {
            use std::os::unix::fs::PermissionsExt;
            let staged = path.with_extension("new");
            std::fs::write(&staged, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::rename(staged, path).unwrap();
        }

        /// A server whose binary is missing is remembered: it is not
        /// retried on its own, a manual restart tries it again, and once
        /// it is installed the manual restart starts it.
        #[test]
        fn a_server_that_failed_to_start_is_started_by_a_manual_restart() {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("server");
            let mut config = LspConfig::default();
            config.servers.insert(
                "test".into(),
                LspServerConfig {
                    command: binary.to_string_lossy().into_owned(),
                    args: Vec::new(),
                    root_markers: Vec::new(),
                },
            );
            let mut manager = LspManager::new(config);
            manager.restart_policy.base_backoff = Duration::from_millis(10);
            let file = dir.path().join("a.rs");
            std::fs::write(&file, "").unwrap();

            assert!(manager.ensure_server("test", &file).is_err());
            assert!(manager.server_start_failed("test", &file));
            assert_eq!(manager.server_status("test", &file), None);
            assert_eq!(manager.server_generation("test", &file), None);

            // Still missing: the manual start fails, and nothing retries it.
            assert!(manager.restart_server("test", &file));
            assert_eq!(
                manager.server_status("test", &file),
                Some(ServerStatus::Restarting)
            );
            poll_until(&mut manager, |m| m.server_start_failed("test", &file));
            for _ in 0..30 {
                assert!(!manager.poll_restarts());
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(manager.server_start_failed("test", &file));

            install(&binary, "echo >> starts; exec sleep 60");
            assert!(manager.restart_server("test", &file));
            let mut went_live = false;
            poll_until(&mut manager, |m| {
                went_live |= m.poll_restarts();
                m.server_generation("test", &file).is_some()
            });
            assert!(went_live, "editors are told to open their documents");
            assert!(!manager.server_start_failed("test", &file));
            // Live means spawned: the script may not have counted itself yet.
            poll_until(&mut manager, |_| starts(dir.path()) > 0);
            assert_eq!(starts(dir.path()), 1);
        }

        /// Opening another file of a server that failed to start tries the
        /// start again, as before the failure was remembered.
        #[test]
        fn opening_a_file_retries_a_failed_start() {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("server");
            let mut config = LspConfig::default();
            config.servers.insert(
                "test".into(),
                LspServerConfig {
                    command: binary.to_string_lossy().into_owned(),
                    args: Vec::new(),
                    root_markers: Vec::new(),
                },
            );
            let mut manager = LspManager::new(config);
            let file = dir.path().join("a.rs");
            std::fs::write(&file, "").unwrap();
            assert!(manager.ensure_server("test", &file).is_err());

            install(&binary, "exec sleep 60");
            manager.ensure_server("test", &file).unwrap();
            assert!(manager.server_generation("test", &file).is_some());
            assert!(!manager.server_start_failed("test", &file));
        }

        /// Restarting a file no server was started for does nothing.
        #[test]
        fn restarting_without_a_server_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let (mut manager, _) = manager(dir.path(), "exec sleep 60");
            let other = tempfile::tempdir().unwrap();
            assert!(!manager.restart_server("test", &other.path().join("b.rs")));
            assert!(!manager.restart_server("python", &dir.path().join("a.py")));
        }
    }
}
