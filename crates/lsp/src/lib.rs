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

mod protocol;
mod server;

pub use lsp_types::CompletionTriggerKind;
pub use protocol::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
pub use server::{LspServer, LspServerConfig, ServerStatus};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, OnceLock};

use anyhow::{Context, Result};
use lsp_types::{
    CompletionResponse, GotoDefinitionResponse, Hover, Location, Position,
    PublishDiagnosticsParams, TextDocumentContentChangeEvent, Uri, WorkspaceEdit,
};
use url::Url;

/// Convert a file path to an LSP Uri. Used across the crate (lib, server).
pub(crate) fn path_to_uri(path: &Path) -> Option<Uri> {
    let url = Url::from_file_path(path).ok()?;
    url.as_str().parse().ok()
}

/// Configuration for LSP servers
#[derive(Debug, Clone, Default)]
pub struct LspConfig {
    /// Per-language server configurations
    pub servers: HashMap<String, LspServerConfig>,
}

/// Manages multiple LSP servers (one per language/workspace)
pub struct LspManager {
    /// Active servers keyed by (language_id, workspace_root)
    servers: HashMap<(String, PathBuf), LspServer>,
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
        Self {
            servers: HashMap::new(),
            config,
            diagnostics_rx,
            diagnostics_tx,
            apply_edit_rx,
            apply_edit_tx,
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
        // Extensions below keep matching exactly as they did before.
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

    /// Ensure a server is running for the given language and file
    pub fn ensure_server(&mut self, lang: &str, file_path: &Path) -> Result<()> {
        let server_config = self
            .config
            .servers
            .get(lang)
            .context(format!("No LSP server configured for language: {}", lang))?
            .clone();

        let workspace_root = Self::find_workspace_root(file_path, &server_config.root_markers)
            .unwrap_or_else(|| file_path.parent().unwrap_or(Path::new("/")).to_path_buf());

        let key = (lang.to_string(), workspace_root.clone());

        if self.servers.contains_key(&key) {
            return Ok(());
        }

        log::info!("Starting LSP server for {} in {:?}", lang, workspace_root);

        let server = LspServer::start(
            lang.to_string(),
            server_config,
            workspace_root,
            self.diagnostics_tx.clone(),
            self.apply_edit_tx.clone(),
        )?;

        self.servers.insert(key, server);
        Ok(())
    }

    /// Get server for language and file (if running)
    fn get_server(&self, lang: &str, file_path: &Path) -> Option<&LspServer> {
        let server_config = self.config.servers.get(lang)?;
        let workspace_root = Self::find_workspace_root(file_path, &server_config.root_markers)
            .unwrap_or_else(|| file_path.parent().unwrap_or(Path::new("/")).to_path_buf());

        let key = (lang.to_string(), workspace_root);
        self.servers.get(&key)
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
        let server = self.get_server(lang, file_path)?;
        let uri = path_to_uri(file_path)?;
        Some(server.completion(uri, position, trigger_kind, trigger_character))
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
        let server = self.get_server(lang, file_path)?;
        let uri = path_to_uri(file_path)?;
        Some(server.code_action(uri, range, diagnostics))
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
        let server = self.get_server(lang, file_path)?;
        let uri = path_to_uri(file_path)?;
        Some(server.hover(uri, position))
    }

    /// Request go-to-definition at position
    pub fn goto_definition(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Option<mpsc::Receiver<Option<GotoDefinitionResponse>>> {
        let server = self.get_server(lang, file_path)?;
        let uri = path_to_uri(file_path)?;
        Some(server.goto_definition(uri, position))
    }

    /// Request find-references at position
    pub fn references(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
        include_declaration: bool,
    ) -> Option<mpsc::Receiver<Option<Vec<Location>>>> {
        let server = self.get_server(lang, file_path)?;
        let uri = path_to_uri(file_path)?;
        Some(server.references(uri, position, include_declaration))
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
    pub fn did_open(&self, lang: &str, file_path: &Path, text: &str) {
        if let Some(server) = self.get_server(lang, file_path) {
            if let Some(uri) = path_to_uri(file_path) {
                server.did_open(uri, lang.to_string(), text.to_string());
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
        self.get_server(lang, file_path).map(|s| s.status())
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

    /// Shutdown all servers
    pub fn shutdown(&mut self) {
        for (key, server) in self.servers.drain() {
            log::info!("Shutting down LSP server for {:?}", key);
            server.shutdown();
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
}
