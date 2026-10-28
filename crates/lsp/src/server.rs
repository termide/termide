//! LSP server process management.
//!
//! Handles spawning the language server, communication via stdin/stdout,
//! and routing requests/responses through channels.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicU64;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lsp_types::{
    ClientCapabilities, CompletionContext, CompletionResponse, CompletionTriggerKind,
    GotoDefinitionResponse, Hover, InitializeParams, InitializeResult, Location, Position,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentClientCapabilities,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
    VersionedTextDocumentIdentifier, WorkspaceEdit,
};
use serde_json::Value;

use crate::path_to_uri;
use crate::protocol::RequestId;

// JSON-RPC transport layer (reader/dispatch loops, request/notification framing).
mod transport;

/// Configuration for a specific LSP server
#[derive(Debug, Clone, Default)]
pub struct LspServerConfig {
    /// Command to start the server
    pub command: String,
    /// Command arguments
    pub args: Vec<String>,
    /// File patterns to identify project root
    pub root_markers: Vec<String>,
}

/// The status a server shows: `Indexing` without a progress token in flight
/// is `Running`, since nothing is being indexed.
fn effective_status(status: ServerStatus, in_progress: bool) -> ServerStatus {
    match status {
        ServerStatus::Indexing if !in_progress => ServerStatus::Running,
        other => other,
    }
}

/// Server status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerStatus {
    Starting,
    Indexing, // Initialized but background indexing in progress
    Running,
    ShuttingDown,
    Stopped,
    /// The connection was lost without a shutdown: the process exited, its
    /// output closed, or it sent a stream that cannot be framed. Requests
    /// fail at once from then on.
    Crashed,
}

/// Requests waiting for their reply, keyed by id.
#[derive(Default)]
struct Pending {
    waiting: HashMap<RequestId, mpsc::Sender<Value>>,
    /// Set once the connection is lost; no request is accepted after it.
    closed: bool,
}

type PendingRequests = Arc<Mutex<Pending>>;

/// How long shutdown waits for the reply to `shutdown`, then for the process
/// to exit after `exit`, before it kills it. Bounded because shutdown runs
/// on quit and project close, on the UI thread.
const SHUTDOWN_REPLY_TIMEOUT: Duration = Duration::from_secs(1);
const EXIT_TIMEOUT: Duration = Duration::from_secs(1);

/// LSP server instance
pub struct LspServer {
    /// Server process
    process: Child,
    /// Next request ID
    next_id: AtomicU64,
    /// Pending requests waiting for responses
    pending: PendingRequests,
    /// Channel to send messages to the writer thread
    writer_tx: mpsc::Sender<String>,
    /// Server status
    status: Arc<Mutex<ServerStatus>>,
    /// Server capabilities (after initialization)
    capabilities: Arc<Mutex<Option<ServerCapabilities>>>,
    /// Active progress tokens (server is indexing while non-empty)
    active_progress: Arc<Mutex<HashSet<String>>>,
}

impl LspServer {
    /// Start a new LSP server
    pub fn start(
        language_id: String,
        config: LspServerConfig,
        workspace_root: PathBuf,
        diagnostics_tx: mpsc::Sender<PublishDiagnosticsParams>,
        apply_edit_tx: mpsc::Sender<WorkspaceEdit>,
    ) -> Result<Self> {
        let mut process = Command::new(&config.command)
            .args(&config.args)
            .current_dir(&workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                log::error!("LSP: Failed to start {}: {}", config.command, e);
                e
            })
            .with_context(|| format!("Failed to start LSP server: {}", config.command))?;

        let stdin = process.stdin.take().context("Failed to get stdin")?;
        let stdout = process.stdout.take().context("Failed to get stdout")?;
        let stderr = process.stderr.take().context("Failed to get stderr")?;

        // Stderr reader thread - captures server error output to journal
        let stderr_handle = {
            let lang = language_id.clone();
            thread::spawn(move || {
                Self::stderr_loop(stderr, &lang);
            })
        };
        // Detach stderr thread - we don't need to join it
        drop(stderr_handle);

        let pending: PendingRequests = Arc::new(Mutex::new(Pending::default()));
        let status = Arc::new(Mutex::new(ServerStatus::Starting));
        let capabilities = Arc::new(Mutex::new(None));
        let active_progress = Arc::new(Mutex::new(HashSet::new()));

        let (writer_tx, writer_rx) = mpsc::channel::<String>();

        // Reader thread - receives messages from server. Neither it nor the
        // writer is joined: a grandchild still holding the server's stdout
        // would keep the reader blocked past the server's own exit.
        {
            let pending = pending.clone();
            let status = status.clone();
            let capabilities = capabilities.clone();
            let active_progress = active_progress.clone();
            let writer_tx = writer_tx.clone();
            thread::spawn(move || {
                Self::reader_loop(
                    stdout,
                    pending,
                    status,
                    capabilities,
                    active_progress,
                    diagnostics_tx,
                    apply_edit_tx,
                    writer_tx,
                );
            });
        }

        let mut server = Self {
            process,
            next_id: AtomicU64::new(1),
            pending,
            writer_tx,
            status,
            capabilities,
            active_progress,
        };

        // Send initialize request
        let initialize_rx = server.initialize(workspace_root, &config.command)?;

        // Writer thread - sends messages to server
        {
            let status = server.status.clone();
            let pending = server.pending.clone();
            thread::spawn(move || {
                Self::writer_loop(stdin, writer_rx, initialize_rx, status, pending);
            });
        }

        Ok(server)
    }

    /// Send initialize request
    #[allow(deprecated)] // root_uri is deprecated but still widely used
    fn initialize(
        &mut self,
        workspace_root: PathBuf,
        command: &str,
    ) -> Result<mpsc::Receiver<Option<InitializeResult>>> {
        let root_uri = path_to_uri(&workspace_root)
            .ok_or_else(|| anyhow::anyhow!("Invalid workspace path"))?;

        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri),
            capabilities: ClientCapabilities {
                // Tell the server we honor server-initiated `workspace/applyEdit`
                // requests, so command-based quick-fixes (e.g. phpactor's
                // "Import class") will push their edits back to us to apply.
                workspace: Some(lsp_types::WorkspaceClientCapabilities {
                    apply_edit: Some(true),
                    ..Default::default()
                }),
                text_document: Some(TextDocumentClientCapabilities {
                    completion: Some(lsp_types::CompletionClientCapabilities {
                        completion_item: Some(lsp_types::CompletionItemCapability {
                            snippet_support: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    hover: Some(lsp_types::HoverClientCapabilities {
                        content_format: Some(vec![lsp_types::MarkupKind::Markdown]),
                        ..Default::default()
                    }),
                    definition: Some(lsp_types::GotoCapability::default()),
                    synchronization: Some(lsp_types::TextDocumentSyncClientCapabilities {
                        dynamic_registration: Some(false),
                        will_save: Some(false),
                        will_save_wait_until: Some(false),
                        did_save: Some(true),
                    }),
                    publish_diagnostics: Some(lsp_types::PublishDiagnosticsClientCapabilities {
                        related_information: Some(true),
                        ..Default::default()
                    }),
                    // Advertise CodeAction literal support (so servers return
                    // `CodeAction` objects, e.g. "Import class"), but NOT
                    // resolveSupport — that makes servers inline the full
                    // `edit`, which we can apply directly without a separate
                    // `codeAction/resolve` round-trip.
                    code_action: Some(lsp_types::CodeActionClientCapabilities {
                        code_action_literal_support: Some(lsp_types::CodeActionLiteralSupport {
                            code_action_kind: lsp_types::CodeActionKindLiteralSupport {
                                value_set: vec![
                                    "".to_string(),
                                    "quickfix".to_string(),
                                    "refactor".to_string(),
                                    "source".to_string(),
                                ],
                            },
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                window: Some(lsp_types::WindowClientCapabilities {
                    work_done_progress: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            initialization_options: transport::initialization_options(command),
            ..Default::default()
        };

        let rx = self.send_request::<InitializeResult>("initialize", params)?;

        // Set to Indexing - will become Running when all progress tokens
        // complete. A server that already died stays crashed.
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        if *status == ServerStatus::Starting {
            *status = ServerStatus::Indexing;
        }
        drop(status);
        Ok(rx)
    }

    /// Request completion at position
    pub fn completion(
        &self,
        uri: Uri,
        position: Position,
        trigger_kind: CompletionTriggerKind,
        trigger_character: Option<String>,
    ) -> mpsc::Receiver<Option<CompletionResponse>> {
        let params = lsp_types::CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            context: Some(CompletionContext {
                trigger_kind,
                trigger_character,
            }),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.send_request("textDocument/completion", params)
            .unwrap_or_else(|_| {
                let (_, rx) = mpsc::channel();
                rx
            })
    }

    /// Completion trigger characters the server advertised in its
    /// `completionProvider` capability (e.g. `->`/`::` components for PHP).
    /// Empty when the server hasn't reported capabilities yet or advertises
    /// none — callers should fall back to a built-in set.
    pub fn completion_trigger_characters(&self) -> Vec<String> {
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|caps| caps.completion_provider.as_ref())
            .and_then(|provider| provider.trigger_characters.clone())
            .unwrap_or_default()
    }

    /// Request code actions for a range (with the diagnostics overlapping it
    /// as context, so servers can offer quick-fixes like "Import class").
    pub fn code_action(
        &self,
        uri: Uri,
        range: lsp_types::Range,
        diagnostics: Vec<lsp_types::Diagnostic>,
    ) -> mpsc::Receiver<Option<lsp_types::CodeActionResponse>> {
        let params = lsp_types::CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range,
            context: lsp_types::CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.send_request("textDocument/codeAction", params)
            .unwrap_or_else(|_| {
                let (_, rx) = mpsc::channel();
                rx
            })
    }

    /// Whether the server resolves code actions lazily (`codeAction/resolve`).
    /// Some servers return actions without an `edit` and fill it in on resolve.
    pub fn supports_code_action_resolve(&self) -> bool {
        matches!(
            self.capabilities
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(|caps| caps.code_action_provider.as_ref()),
            Some(lsp_types::CodeActionProviderCapability::Options(
                lsp_types::CodeActionOptions {
                    resolve_provider: Some(true),
                    ..
                }
            ))
        )
    }

    /// Resolve a code action that was returned without an inline `edit`,
    /// asking the server to fill it in.
    pub fn code_action_resolve(
        &self,
        action: lsp_types::CodeAction,
    ) -> mpsc::Receiver<Option<lsp_types::CodeAction>> {
        self.send_request("codeAction/resolve", action)
            .unwrap_or_else(|_| {
                let (_, rx) = mpsc::channel();
                rx
            })
    }

    /// Execute a server command (`workspace/executeCommand`).
    ///
    /// Used for command-based code actions (e.g. phpactor "Import class"),
    /// where the server performs the change itself and pushes the resulting
    /// edit back via a `workspace/applyEdit` request. The command's own result
    /// is irrelevant to us, so it is sent fire-and-forget.
    pub fn execute_command(&self, command: String, arguments: Vec<Value>) {
        let params = lsp_types::ExecuteCommandParams {
            command,
            arguments,
            work_done_progress_params: Default::default(),
        };
        let _ = self.send_request::<Value>("workspace/executeCommand", params);
    }

    /// Request hover at position
    pub fn hover(&self, uri: Uri, position: Position) -> mpsc::Receiver<Option<Hover>> {
        let params = lsp_types::HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: Default::default(),
        };

        self.send_request("textDocument/hover", params)
            .unwrap_or_else(|_| {
                let (_, rx) = mpsc::channel();
                rx
            })
    }

    /// Request go-to-definition at position
    pub fn goto_definition(
        &self,
        uri: Uri,
        position: Position,
    ) -> mpsc::Receiver<Option<GotoDefinitionResponse>> {
        let params = lsp_types::GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.send_request("textDocument/definition", params)
            .unwrap_or_else(|_| {
                let (_, rx) = mpsc::channel();
                rx
            })
    }

    /// Request find-references at position
    pub fn references(
        &self,
        uri: Uri,
        position: Position,
        include_declaration: bool,
    ) -> mpsc::Receiver<Option<Vec<Location>>> {
        let params = lsp_types::ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            context: lsp_types::ReferenceContext {
                include_declaration,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        self.send_request("textDocument/references", params)
            .unwrap_or_else(|_| {
                let (_, rx) = mpsc::channel();
                rx
            })
    }

    /// Request rename symbol at position
    pub fn rename(
        &self,
        uri: Uri,
        position: Position,
        new_name: String,
    ) -> mpsc::Receiver<Option<WorkspaceEdit>> {
        let params = lsp_types::RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            new_name,
            work_done_progress_params: Default::default(),
        };

        self.send_request("textDocument/rename", params)
            .unwrap_or_else(|_| {
                let (_, rx) = mpsc::channel();
                rx
            })
    }

    /// Send textDocument/didOpen notification
    pub fn did_open(&self, uri: Uri, language_id: String, text: String) {
        let params = lsp_types::DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id,
                version: 1,
                text,
            },
        };
        self.send_notification("textDocument/didOpen", params);
    }

    /// Whether this server accepts ranged `didChange` notifications.
    ///
    /// A server that advertises `Full` — or has not reported its capabilities
    /// yet — must be sent the whole document; handing it a ranged change would
    /// silently corrupt its copy, and every answer afterwards with it.
    pub fn supports_incremental_sync(&self) -> bool {
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|caps| caps.text_document_sync.as_ref())
            .map(|sync| match sync {
                TextDocumentSyncCapability::Kind(kind) => {
                    *kind == TextDocumentSyncKind::INCREMENTAL
                }
                TextDocumentSyncCapability::Options(options) => {
                    options.change == Some(TextDocumentSyncKind::INCREMENTAL)
                }
            })
            .unwrap_or(false)
    }

    /// Send textDocument/didChange notification with the whole document.
    pub fn did_change_full(&self, uri: Uri, version: i32, text: String) {
        self.did_change(
            uri,
            version,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text,
            }],
        );
    }

    /// Send textDocument/didChange notification.
    ///
    /// Ranged changes are applied in order, each to the document as the
    /// previous one left it. Only send them when
    /// [`Self::supports_incremental_sync`] agrees.
    pub fn did_change(
        &self,
        uri: Uri,
        version: i32,
        content_changes: Vec<TextDocumentContentChangeEvent>,
    ) {
        let params = lsp_types::DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri, version },
            content_changes,
        };
        self.send_notification("textDocument/didChange", params);
    }

    /// Send textDocument/didClose notification
    pub fn did_close(&self, uri: Uri) {
        let params = lsp_types::DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
        };
        self.send_notification("textDocument/didClose", params);
    }

    /// Send textDocument/didSave notification
    ///
    /// This triggers full project analysis in rust-analyzer and other LSP servers,
    /// which is necessary for detecting logical errors like unresolved modules.
    pub fn did_save(&self, uri: Uri, text: Option<String>) {
        let params = lsp_types::DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
            text,
        };
        self.send_notification("textDocument/didSave", params);
    }

    /// Current server status. A server that finished `initialize` stays
    /// `Indexing` only while it reports work in progress; one that sends no
    /// `$/progress` at all (pylsp, for one) is simply running.
    pub fn status(&self) -> ServerStatus {
        let status = *self.status.lock().unwrap_or_else(|e| e.into_inner());
        let in_progress = !self
            .active_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
        effective_status(status, in_progress)
    }

    /// Check if server is ready: running, with no work in progress.
    pub fn is_ready(&self) -> bool {
        self.status() == ServerStatus::Running
    }

    /// Check if server is actively indexing (has active progress tokens)
    pub fn is_indexing(&self) -> bool {
        self.status() == ServerStatus::Indexing
    }

    /// Shut the server down: `shutdown`, then `exit`, then the process is
    /// reaped — killed if it has not exited by then. Takes at most about
    /// two seconds, however the server behaves.
    pub fn shutdown(self) {
        self.shutdown_within(SHUTDOWN_REPLY_TIMEOUT, EXIT_TIMEOUT);
    }

    fn shutdown_within(mut self, reply_timeout: Duration, exit_timeout: Duration) {
        // Keeps a lost connection from being reported as a crash; the writer
        // still writes what is queued.
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = ServerStatus::ShuttingDown;

        // A dead server fails this at once, and its receiver disconnects as
        // soon as the connection is lost, so neither waits out the timeout.
        if let Ok(reply) = self.send_request::<Value>("shutdown", Value::Null) {
            let _ = reply.recv_timeout(reply_timeout);
        }
        self.send_notification("exit", Value::Null);

        let deadline = Instant::now() + exit_timeout;
        loop {
            match self.process.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = self.process.kill();
                    let _ = self.process.wait();
                    break;
                }
            }
        }

        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = ServerStatus::Stopped;
    }
}

#[cfg(test)]
mod tests {
    use super::{effective_status, ServerStatus};

    /// A server that never reports progress must not look busy forever.
    #[test]
    fn indexing_lasts_only_while_progress_is_reported() {
        assert_eq!(
            effective_status(ServerStatus::Indexing, false),
            ServerStatus::Running
        );
        assert_eq!(
            effective_status(ServerStatus::Indexing, true),
            ServerStatus::Indexing
        );
        for status in [
            ServerStatus::Starting,
            ServerStatus::Running,
            ServerStatus::ShuttingDown,
            ServerStatus::Stopped,
            ServerStatus::Crashed,
        ] {
            assert_eq!(effective_status(status, false), status);
        }
    }

    /// Stand-in servers written in `sh`, so the process side is real.
    #[cfg(unix)]
    mod process {
        use std::path::Path;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        use crate::server::{LspServer, LspServerConfig, ServerStatus};

        fn start(dir: &Path, script: &str) -> LspServer {
            let (diagnostics_tx, _) = mpsc::channel();
            let (apply_edit_tx, _) = mpsc::channel();
            let config = LspServerConfig {
                command: "sh".into(),
                args: vec!["-c".into(), script.into()],
                root_markers: Vec::new(),
            };
            LspServer::start(
                "test".into(),
                config,
                dir.to_path_buf(),
                diagnostics_tx,
                apply_edit_tx,
            )
            .expect("sh starts")
        }

        fn hover(server: &LspServer, dir: &Path) -> mpsc::Receiver<Option<lsp_types::Hover>> {
            let uri = crate::path_to_uri(&dir.join("a.rs")).unwrap();
            server.hover(uri, lsp_types::Position::new(0, 0))
        }

        fn wait_for(server: &LspServer, status: ServerStatus) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while server.status() != status {
                assert!(Instant::now() < deadline, "still {:?}", server.status());
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn pending_count(server: &LspServer) -> usize {
            server.pending.lock().unwrap().waiting.len()
        }

        /// A server that dies shows as crashed instead of running forever,
        /// and a request already waiting is released rather than holding its
        /// thread for good.
        #[test]
        fn a_dead_server_is_crashed_and_releases_waiting_requests() {
            let dir = tempfile::tempdir().unwrap();
            // Alive long enough to take a request, never answering it.
            let server = start(dir.path(), "sleep 0.3");
            let waiting = hover(&server, dir.path());

            wait_for(&server, ServerStatus::Crashed);
            assert_eq!(
                waiting.recv_timeout(Duration::from_secs(5)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            );
            assert_eq!(pending_count(&server), 0);
            server.shutdown_within(Duration::from_secs(5), Duration::from_secs(5));
        }

        /// Every request to a crashed server fails at once: nothing is left
        /// in `pending`, no thread waits on a reply that cannot come.
        #[test]
        fn requests_to_a_crashed_server_fail_fast() {
            let dir = tempfile::tempdir().unwrap();
            let server = start(dir.path(), "exit 0");
            wait_for(&server, ServerStatus::Crashed);

            for _ in 0..50 {
                let rx = hover(&server, dir.path());
                assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
            }
            assert_eq!(pending_count(&server), 0);

            // Nothing left to wait for, so no timeout is waited out either.
            let start = Instant::now();
            server.shutdown_within(Duration::from_secs(5), Duration::from_secs(5));
            assert!(start.elapsed() < Duration::from_secs(2));
        }

        /// A server that ignores both `exit` and the end of its input is
        /// killed after the bounded waits: quitting must never hang on it.
        #[test]
        fn shutdown_is_bounded_for_a_server_that_will_not_exit() {
            let dir = tempfile::tempdir().unwrap();
            let server = start(dir.path(), "trap '' TERM HUP; exec sleep 60");

            let start = Instant::now();
            server.shutdown_within(Duration::from_millis(200), Duration::from_millis(200));
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "took {:?}",
                start.elapsed()
            );
        }

        /// `shutdown` and `exit` reach the server: the writer no longer stops
        /// at the status change that comes before them.
        #[test]
        fn shutdown_and_exit_are_written_to_the_server() {
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("received");
            let reply = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
            // Answer `initialize`, then record everything that comes in.
            let script = format!(
                "printf 'Content-Length: {}\\r\\n\\r\\n%s' '{reply}'; exec cat > '{}'",
                reply.len(),
                out.display()
            );
            let server = start(dir.path(), &script);
            wait_for(&server, ServerStatus::Running);

            server.shutdown_within(Duration::from_millis(300), Duration::from_millis(300));
            let received = std::fs::read_to_string(&out).unwrap();
            let at = |method: &str| {
                received
                    .find(&format!(r#""method":"{method}""#))
                    .unwrap_or_else(|| panic!("no {method} in {received}"))
            };
            assert!(at("initialize") < at("initialized"));
            assert!(at("initialized") < at("shutdown"));
            assert!(at("shutdown") < at("exit"));
        }
    }
}
