//! JSON-RPC transport for the LSP server: the reader/stderr loops, inbound
//! message dispatch, request-id correlation, and outbound request/notification
//! framing. The typed feature API (`completion`, `hover`, …) lives in the
//! parent `server` module and drives this layer through `send_request` /
//! `send_notification`.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::process::ChildStdin;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use lsp_types::{InitializeResult, PublishDiagnosticsParams, ServerCapabilities, WorkspaceEdit};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::protocol::{
    decode_header, encode_message, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId,
};

use super::{LspServer, PendingRequests, ServerStatus};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);

/// `initializationOptions` a server needs at `initialize` to opt out of
/// behaviour we do not want on its default. Keyed by the server's command:
/// this document has no meaning outside the server it was written for, so
/// each entry states the option its server actually reads. Only the file
/// name is compared, so an absolute path or a Windows `.exe` still matches.
pub(super) fn initialization_options(command: &str) -> Option<Value> {
    let name = std::path::Path::new(command).file_name()?.to_str()?;
    let name = name.strip_suffix(".exe").unwrap_or(name);
    match name {
        // docker-language-server reports to BugSnag with telemetry set to
        // `all` unless the `initialize` request says otherwise, and
        // `initialize` is the only place it will hear us: it reads telemetry
        // from these options and, failing that, from a
        // `workspace/configuration` round-trip we never start — answering
        // that falls back to `all` again.
        "docker-language-server" => Some(serde_json::json!({ "telemetry": "off" })),
        _ => None,
    }
}

impl LspServer {
    pub(super) fn writer_loop(
        mut stdin: ChildStdin,
        writer_rx: mpsc::Receiver<String>,
        initialize_rx: mpsc::Receiver<Option<InitializeResult>>,
        status: Arc<Mutex<ServerStatus>>,
    ) {
        let mut initialize_rx = Some(initialize_rx);
        while let Ok(msg) = writer_rx.recv() {
            if Self::is_shutting_down(&status) || !Self::write_message(&mut stdin, &msg) {
                break;
            }
            if let Some(initialize_rx) = initialize_rx.take() {
                if !Self::await_initialize(&initialize_rx, &status, INITIALIZE_TIMEOUT) {
                    break;
                }
                let initialized =
                    JsonRpcNotification::new("initialized", Some(serde_json::json!({})));
                let Ok(initialized) = encode_message(&initialized) else {
                    break;
                };
                if !Self::write_message(&mut stdin, &initialized) {
                    break;
                }
            }
        }
    }

    /// Wait for the `initialize` response before letting `initialized` go out.
    /// `true` means "send `initialized`": the response arrived, the channel
    /// died, or the wait ran out — the last being the old degraded behaviour,
    /// preferred to a writer held forever. `false` means shut down instead.
    ///
    /// `timeout` is a parameter rather than the constant so the timeout path
    /// can be tested without costing 30 seconds.
    fn await_initialize(
        initialize_rx: &mpsc::Receiver<Option<InitializeResult>>,
        status: &Arc<Mutex<ServerStatus>>,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match initialize_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if Self::is_shutting_down(status) {
                        return false;
                    }
                    if Instant::now() >= deadline {
                        log::warn!("LSP: no response to initialize, sending initialized anyway");
                        return true;
                    }
                }
            }
        }
    }

    fn is_shutting_down(status: &Arc<Mutex<ServerStatus>>) -> bool {
        *status.lock().unwrap_or_else(|e| e.into_inner()) == ServerStatus::ShuttingDown
    }

    fn write_message(stdin: &mut ChildStdin, msg: &str) -> bool {
        if let Err(e) = stdin.write_all(msg.as_bytes()) {
            log::error!("Failed to write to LSP server: {}", e);
            return false;
        }
        if let Err(e) = stdin.flush() {
            log::error!("Failed to flush LSP server stdin: {}", e);
            return false;
        }
        true
    }

    /// Reader thread main loop
    #[allow(clippy::too_many_arguments)]
    pub(super) fn reader_loop(
        stdout: std::process::ChildStdout,
        pending: PendingRequests,
        status: Arc<Mutex<ServerStatus>>,
        capabilities: Arc<Mutex<Option<ServerCapabilities>>>,
        active_progress: Arc<Mutex<HashSet<String>>>,
        diagnostics_tx: mpsc::Sender<PublishDiagnosticsParams>,
        apply_edit_tx: mpsc::Sender<WorkspaceEdit>,
        writer_tx: mpsc::Sender<String>,
    ) {
        let mut reader = BufReader::new(stdout);
        let mut header = String::new();

        loop {
            if *status.lock().unwrap_or_else(|e| e.into_inner()) == ServerStatus::ShuttingDown {
                break;
            }

            header.clear();

            // Read headers until empty line
            let mut content_length = 0;
            loop {
                header.clear();
                match reader.read_line(&mut header) {
                    Ok(0) => return, // EOF
                    Ok(_) => {
                        let trimmed = header.trim();
                        if trimmed.is_empty() {
                            break; // End of headers
                        }
                        if let Some(len) = decode_header(trimmed) {
                            content_length = len;
                        }
                    }
                    Err(e) => {
                        log::error!("Error reading LSP header: {}", e);
                        return;
                    }
                }
            }

            if content_length == 0 {
                continue;
            }

            // Read content
            let mut content = vec![0u8; content_length];
            if let Err(e) = std::io::Read::read_exact(&mut reader, &mut content) {
                log::error!("Error reading LSP content: {}", e);
                return;
            }

            // Parse message
            let content_str = match String::from_utf8(content) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Invalid UTF-8 in LSP message: {}", e);
                    continue;
                }
            };

            match serde_json::from_str::<JsonRpcMessage>(&content_str) {
                Ok(JsonRpcMessage::Response(response)) => {
                    Self::handle_response(&pending, &capabilities, response);
                }
                Ok(JsonRpcMessage::Notification(notification)) => {
                    Self::handle_notification(
                        &diagnostics_tx,
                        &status,
                        &active_progress,
                        notification,
                    );
                }
                Ok(JsonRpcMessage::Request(request)) => {
                    // Server-initiated requests (e.g. workspace/applyEdit)
                    Self::handle_server_request(&writer_tx, &apply_edit_tx, request);
                }
                Err(e) => {
                    log::error!("Failed to parse LSP message: {}", e);
                }
            }
        }
    }

    /// Stderr reader loop - captures server error output to journal
    pub(super) fn stderr_loop(stderr: std::process::ChildStderr, lang: &str) {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) if !line.is_empty() => {
                    // Log to journal as warning (not error, since LSP servers often
                    // write informational messages to stderr)
                    log::warn!("LSP [{}]: {}", lang, line);
                }
                Err(_) => break,
                _ => {}
            }
        }
    }

    /// Handle a response from the server
    fn handle_response(
        pending: &PendingRequests,
        capabilities: &Arc<Mutex<Option<ServerCapabilities>>>,
        response: JsonRpcResponse,
    ) {
        // Find and notify the waiting request
        let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = pending.remove(&response.id) {
            if let Some(result) = response.result {
                // Try to parse as InitializeResult before sending
                // This avoids cloning the entire JSON value
                if let Ok(init_result) = serde_json::from_value::<InitializeResult>(result.clone())
                {
                    *capabilities.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(init_result.capabilities);
                }
                let _ = tx.send(result);
            } else if let Some(error) = response.error {
                log::warn!("LSP error {}: {}", error.code, error.message);
            }
        } else if let Some(result) = response.result {
            // No pending request - might be initialize response
            if let Ok(init_result) = serde_json::from_value::<InitializeResult>(result) {
                *capabilities.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(init_result.capabilities);
            }
        }
    }

    /// Handle a notification from the server
    fn handle_notification(
        diagnostics_tx: &mpsc::Sender<PublishDiagnosticsParams>,
        status: &Arc<Mutex<ServerStatus>>,
        active_progress: &Arc<Mutex<HashSet<String>>>,
        notification: JsonRpcNotification,
    ) {
        match notification.method.as_str() {
            "textDocument/publishDiagnostics" => {
                if let Some(params) = notification.params {
                    if let Ok(diagnostics) = serde_json::from_value(params) {
                        let _ = diagnostics_tx.send(diagnostics);
                    }
                }
            }
            "$/progress" => {
                if let Some(params) = notification.params {
                    Self::handle_progress(status, active_progress, params);
                }
            }
            "window/logMessage" | "window/showMessage" => {
                // Log server messages
            }
            _ => {}
        }
    }

    /// Handle $/progress notification
    fn handle_progress(
        status: &Arc<Mutex<ServerStatus>>,
        active_progress: &Arc<Mutex<HashSet<String>>>,
        params: Value,
    ) {
        use lsp_types::{NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress};

        if let Ok(progress) = serde_json::from_value::<ProgressParams>(params) {
            let token = match &progress.token {
                NumberOrString::String(s) => s.clone(),
                NumberOrString::Number(n) => n.to_string(),
            };

            match progress.value {
                ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(_begin)) => {
                    active_progress
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(token);
                    *status.lock().unwrap_or_else(|e| e.into_inner()) = ServerStatus::Indexing;
                }
                ProgressParamsValue::WorkDone(WorkDoneProgress::Report(_report)) => {
                    // Optional: could show percentage if available
                }
                ProgressParamsValue::WorkDone(WorkDoneProgress::End(_)) => {
                    let mut progress = active_progress.lock().unwrap_or_else(|e| e.into_inner());
                    progress.remove(&token);
                    if progress.is_empty() {
                        drop(progress); // Release before acquiring status lock
                        *status.lock().unwrap_or_else(|e| e.into_inner()) = ServerStatus::Running;
                    }
                }
            }
        }
    }

    /// Handle a server-initiated request.
    ///
    /// `workspace/applyEdit` is forwarded to the app layer (which owns the
    /// buffers) and acknowledged as applied — this is the path command-based
    /// quick-fixes use, e.g. phpactor's "Import class", which performs the edit
    /// via `workspace/executeCommand` and pushes the resulting edit back here.
    /// Every other server request gets a null result.
    fn handle_server_request(
        writer_tx: &mpsc::Sender<String>,
        apply_edit_tx: &mpsc::Sender<WorkspaceEdit>,
        request: JsonRpcRequest,
    ) {
        let result = if request.method == "workspace/applyEdit" {
            let edit = request
                .params
                .and_then(|p| serde_json::from_value::<lsp_types::ApplyWorkspaceEditParams>(p).ok())
                .map(|params| params.edit);

            let applied = match edit {
                Some(edit) => apply_edit_tx.send(edit).is_ok(),
                None => false,
            };

            serde_json::to_value(lsp_types::ApplyWorkspaceEditResponse {
                applied,
                failure_reason: None,
                failed_change: None,
            })
            .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        };

        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: Some(result),
            error: None,
        };

        if let Ok(msg) = encode_message(&response) {
            let _ = writer_tx.send(msg);
        }
    }

    /// Generate next request ID
    fn next_request_id(&self) -> RequestId {
        RequestId::Number(self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    /// Send a request and return a receiver for the response
    pub(super) fn send_request<T: DeserializeOwned + Send + 'static>(
        &self,
        method: &str,
        params: impl serde::Serialize,
    ) -> Result<mpsc::Receiver<Option<T>>> {
        let id = self.next_request_id();
        let request = JsonRpcRequest::new(id.clone(), method, Some(serde_json::to_value(params)?));

        let (tx, rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();

        // Store the sender for when response arrives
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);

        // Spawn thread to convert Value to T
        thread::spawn(move || {
            if let Ok(value) = rx.recv() {
                let result = serde_json::from_value::<T>(value).ok();
                let _ = result_tx.send(result);
            }
        });

        // Send the request
        let msg = encode_message(&request)?;
        self.writer_tx.send(msg)?;

        Ok(result_rx)
    }

    /// Send a notification (no response expected)
    pub(super) fn send_notification(&self, method: &str, params: impl serde::Serialize) {
        let notification = JsonRpcNotification::new(method, serde_json::to_value(params).ok());
        if let Ok(msg) = encode_message(&notification) {
            let _ = self.writer_tx.send(msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_server_is_told_to_send_no_telemetry() {
        // docker-language-server defaults to `all`; without this it posts
        // crash stacks and path hashes to BugSnag on the user's dime.
        let options = initialization_options("docker-language-server").expect("options");
        assert_eq!(options["telemetry"], "off");
    }

    #[test]
    fn docker_server_given_by_path_is_told_to_send_no_telemetry() {
        // A user entry may name the binary by path; the opt-out must follow.
        for command in [
            "/usr/local/bin/docker-language-server",
            "bin/docker-language-server",
            "docker-language-server.exe",
        ] {
            let options = initialization_options(command).expect("options");
            assert_eq!(options["telemetry"], "off", "{command}");
        }
        assert_eq!(initialization_options("/usr/bin/gopls"), None);
        assert_eq!(initialization_options(""), None);
    }

    #[test]
    fn other_servers_get_no_initialization_options() {
        // None of these read a `telemetry` option; sending them a document
        // written for another server is at best noise we did not justify.
        for command in [
            "rust-analyzer",
            "pylsp",
            "typescript-language-server",
            "gopls",
            "terraform-ls",
            "phpantom_lsp",
        ] {
            assert_eq!(
                initialization_options(command),
                None,
                "{command} must initialize with no options"
            );
        }
    }

    /// A server that answers `initialize` releases the writer at once, so the
    /// queued `initialized` and everything behind it flow.
    #[test]
    fn await_initialize_returns_as_soon_as_the_response_arrives() {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(ServerStatus::Indexing));
        tx.send(None).unwrap();

        let start = Instant::now();
        assert!(LspServer::await_initialize(
            &rx,
            &status,
            INITIALIZE_TIMEOUT
        ));
        assert!(
            start.elapsed() < INITIALIZE_TIMEOUT / 2,
            "should not have waited for the deadline"
        );
    }

    /// Dropping the sender means the response was consumed or lost; the writer
    /// must not hang on it.
    #[test]
    fn await_initialize_passes_on_a_disconnected_channel() {
        let (tx, rx) = mpsc::channel::<Option<InitializeResult>>();
        let status = Arc::new(Mutex::new(ServerStatus::Indexing));
        drop(tx);

        assert!(LspServer::await_initialize(
            &rx,
            &status,
            INITIALIZE_TIMEOUT
        ));
    }

    /// Shutdown during the wait stops it: closing a panel must not have to sit
    /// through the timeout.
    #[test]
    fn await_initialize_stops_on_shutdown() {
        let (_tx, rx) = mpsc::channel::<Option<InitializeResult>>();
        let status = Arc::new(Mutex::new(ServerStatus::Indexing));
        *status.lock().unwrap() = ServerStatus::ShuttingDown;

        let start = Instant::now();
        assert!(
            !LspServer::await_initialize(&rx, &status, INITIALIZE_TIMEOUT),
            "a server going away mid-initialize is not a pass"
        );
        assert!(
            start.elapsed() < INITIALIZE_TIMEOUT / 2,
            "shutdown was noticed, not the deadline"
        );
    }

    /// A server that never answers gives up and sends `initialized` anyway —
    /// the old degraded behaviour, preferred to a writer held forever. Costs
    /// the poll interval, not the 30s the writer waits for.
    #[test]
    fn await_initialize_passes_when_no_response_arrives() {
        let (_tx, rx) = mpsc::channel::<Option<InitializeResult>>();
        let status = Arc::new(Mutex::new(ServerStatus::Indexing));

        assert!(LspServer::await_initialize(
            &rx,
            &status,
            Duration::from_millis(150)
        ));
    }

    #[test]
    fn apply_edit_request_forwards_edit_and_acks_applied() {
        let (writer_tx, writer_rx) = mpsc::channel();
        let (apply_tx, apply_rx) = mpsc::channel();
        let params = lsp_types::ApplyWorkspaceEditParams {
            label: Some("Import class".into()),
            edit: WorkspaceEdit::default(),
        };
        let request = JsonRpcRequest::new(
            7u64,
            "workspace/applyEdit",
            Some(serde_json::to_value(params).unwrap()),
        );

        LspServer::handle_server_request(&writer_tx, &apply_tx, request);

        // The edit is forwarded to the app layer for application...
        assert!(apply_rx.try_recv().is_ok());
        // ...and the server is told it was applied.
        let msg = writer_rx.try_recv().unwrap();
        assert!(msg.contains("\"applied\":true"));
        assert!(msg.contains("\"id\":7"));
    }

    #[test]
    fn other_server_request_gets_null_result_and_no_edit() {
        let (writer_tx, writer_rx) = mpsc::channel();
        let (apply_tx, apply_rx) = mpsc::channel();
        let request =
            JsonRpcRequest::new(3u64, "window/workDoneProgress/create", Some(Value::Null));

        LspServer::handle_server_request(&writer_tx, &apply_tx, request);

        // Nothing is forwarded for application, and a null result is returned.
        assert!(apply_rx.try_recv().is_err());
        let msg = writer_rx.try_recv().unwrap();
        assert!(msg.contains("\"result\":null"));
    }
}
