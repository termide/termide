//! LSP server process management.
//!
//! Handles spawning the language server, communication via stdin/stdout,
//! and routing requests/responses through channels.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicU64;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use lsp_types::{
    ClientCapabilities, CompletionContext, CompletionResponse, CompletionTriggerKind,
    GotoDefinitionResponse, Hover, InitializeParams, InitializeResult, Location, Position,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentClientCapabilities,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, VersionedTextDocumentIdentifier, WorkspaceEdit,
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

/// Server status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerStatus {
    Starting,
    Indexing, // Initialized but background indexing in progress
    Running,
    ShuttingDown,
    Stopped,
}

/// Pending request tracking
type PendingRequests = Arc<Mutex<HashMap<RequestId, mpsc::Sender<Value>>>>;

/// LSP server instance
#[allow(dead_code)] // Fields used once didOpen/didChange/completion are wired up
pub struct LspServer {
    /// Language ID
    language_id: String,
    /// Workspace root
    workspace_root: PathBuf,
    /// Server process
    process: Child,
    /// Next request ID
    next_id: AtomicU64,
    /// Pending requests waiting for responses
    pending: PendingRequests,
    /// Writer thread handle
    writer_handle: Option<JoinHandle<()>>,
    /// Reader thread handle
    reader_handle: Option<JoinHandle<()>>,
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

        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let status = Arc::new(Mutex::new(ServerStatus::Starting));
        let capabilities = Arc::new(Mutex::new(None));
        let active_progress = Arc::new(Mutex::new(HashSet::new()));

        // Writer thread - sends messages to server
        let (writer_tx, writer_rx) = mpsc::channel::<String>();
        let writer_handle = {
            let status = status.clone();
            thread::spawn(move || {
                let mut stdin = stdin;
                while let Ok(msg) = writer_rx.recv() {
                    if *status.lock().unwrap_or_else(|e| e.into_inner())
                        == ServerStatus::ShuttingDown
                    {
                        break;
                    }
                    if let Err(e) = stdin.write_all(msg.as_bytes()) {
                        log::error!("Failed to write to LSP server: {}", e);
                        break;
                    }
                    if let Err(e) = stdin.flush() {
                        log::error!("Failed to flush LSP server stdin: {}", e);
                        break;
                    }
                }
            })
        };

        // Reader thread - receives messages from server
        let reader_handle = {
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
            })
        };

        let mut server = Self {
            language_id,
            workspace_root: workspace_root.clone(),
            process,
            next_id: AtomicU64::new(1),
            pending,
            writer_handle: Some(writer_handle),
            reader_handle: Some(reader_handle),
            writer_tx,
            status,
            capabilities,
            active_progress,
        };

        // Send initialize request
        server.initialize(workspace_root)?;

        Ok(server)
    }

    /// Send initialize request
    #[allow(deprecated)] // root_uri is deprecated but still widely used
    fn initialize(&mut self, workspace_root: PathBuf) -> Result<()> {
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
            ..Default::default()
        };

        let _rx = self.send_request::<InitializeResult>("initialize", params)?;

        // Send initialized notification
        self.send_notification("initialized", serde_json::json!({}));

        // Set to Indexing - will become Running when all progress tokens complete
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = ServerStatus::Indexing;
        Ok(())
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

    /// Send textDocument/didChange notification (full sync)
    pub fn did_change(&self, uri: Uri, version: i32, text: String) {
        let params = lsp_types::DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri, version },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text,
            }],
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

    /// Get current server status
    pub fn status(&self) -> ServerStatus {
        *self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Check if server is effectively ready (Running, or Indexing with no active progress)
    pub fn is_ready(&self) -> bool {
        let status = *self.status.lock().unwrap_or_else(|e| e.into_inner());
        match status {
            ServerStatus::Running => true,
            ServerStatus::Indexing => {
                // If no active progress, consider it ready
                // (server might not support/send progress notifications)
                self.active_progress
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty()
            }
            _ => false,
        }
    }

    /// Check if server is actively indexing (has active progress tokens)
    pub fn is_indexing(&self) -> bool {
        let status = *self.status.lock().unwrap_or_else(|e| e.into_inner());
        status == ServerStatus::Indexing
            && !self
                .active_progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
    }

    /// Shutdown the server
    pub fn shutdown(mut self) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = ServerStatus::ShuttingDown;

        // Send shutdown request
        let _ = self.send_request::<()>("shutdown", serde_json::json!(null));

        // Send exit notification
        self.send_notification("exit", serde_json::json!(null));

        // Wait for threads to finish
        if let Some(handle) = self.writer_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }

        // Kill process if still running
        let _ = self.process.kill();
        let _ = self.process.wait();

        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = ServerStatus::Stopped;
    }
}
