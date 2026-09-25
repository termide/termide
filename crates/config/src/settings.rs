//! Configuration structures for termide settings.

use serde::{Deserialize, Serialize};

use crate::defaults;

/// Icon rendering mode for panel titles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconMode {
    /// Auto-detect based on terminal capabilities
    #[default]
    Auto,
    /// Force emoji icons
    Emoji,
    /// Unicode-only mode (no emoji, no arrows)
    Unicode,
}
use crate::keybindings::{
    DatabaseKeybindings, EditorKeybindings, FileManagerKeybindings, GitDiffKeybindings,
    GitLogKeybindings, GitStatusKeybindings, GlobalKeybindings, TerminalKeybindings,
    ViewerKeybindings,
};

/// The context window used when `[ai] context_window_fallback` is unset and
/// the provider does not report a model's window.
pub const DEFAULT_CONTEXT_WINDOW_FALLBACK: u64 = 32_000;

/// Application configuration with nested sections.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// General application settings
    #[serde(default)]
    pub general: GeneralSettings,

    /// Editor settings
    #[serde(default)]
    pub editor: EditorSettings,

    /// File manager settings
    #[serde(default)]
    pub file_manager: FileManagerSettings,

    /// Git status panel settings
    #[serde(default)]
    pub git_status: GitStatusSettings,

    /// Git diff panel settings
    #[serde(default)]
    pub git_diff: GitDiffSettings,

    /// Git log panel settings
    #[serde(default)]
    pub git_log: GitLogSettings,

    /// Database viewer panel settings
    #[serde(default)]
    pub database: DatabaseSettings,

    /// File viewer panels (binary hex, markdown) settings
    #[serde(default)]
    pub viewer: ViewerSettings,

    /// Terminal panel settings
    #[serde(default)]
    pub terminal: TerminalSettings,

    /// LSP settings
    #[serde(default)]
    pub lsp: LspSettings,

    /// Logging settings
    #[serde(default)]
    pub logging: LoggingSettings,

    /// VFS (network filesystem) settings
    #[serde(default)]
    pub vfs: VfsSettings,

    /// Syntax-highlighting settings (custom keyword languages)
    #[serde(default)]
    pub highlight: HighlightSettings,

    /// AI settings (model access and the coding agent panel).
    #[serde(default)]
    pub ai: AiSettings,
}

/// AI settings: which model to talk to and what it may do.
///
/// The provider is any OpenAI-compatible endpoint, which covers local
/// servers (llama.cpp, Ollama, vLLM, omlx) and most gateways. The API key is
/// read from `api_key_env` rather than stored here, so the config file never
/// holds a secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSettings {
    /// Wire protocol: `openai_compatible` (the default, for omlx, OpenAI,
    /// OpenRouter and most gateways) or `anthropic_compatible` (the Messages
    /// API). The value is the protocol; `base_url` picks the actual endpoint.
    #[serde(default = "agent_defaults::provider")]
    pub provider: String,

    /// Base URL including the API prefix, e.g. `http://127.0.0.1:10000/v1`.
    /// For `anthropic_compatible` it is left at the default unless a gateway is
    /// used.
    #[serde(default = "agent_defaults::base_url")]
    pub base_url: String,

    /// Model id as the endpoint expects it. Empty disables the panel.
    #[serde(default)]
    pub model: String,

    /// Environment variable holding the API key; empty for local servers.
    #[serde(default = "agent_defaults::api_key_env")]
    pub api_key_env: String,

    /// Fallback context window in tokens, used only when the provider does not
    /// report a model's window (a local server's `max_model_len`). Unset falls
    /// back to [`agent_defaults::context_window`]. The provider's reported
    /// window always wins.
    #[serde(default)]
    pub context_window_fallback: Option<u64>,

    /// Upper bound on the model's output tokens per turn (one response). Zero
    /// or a negative number sets no bound and leaves the length to the model.
    #[serde(default = "agent_defaults::max_tokens")]
    pub max_tokens_per_turn: i64,

    /// Prefer reasoning: request `reasoning_effort` / extended thinking from
    /// models that support it (ignored by models that do not).
    #[serde(default)]
    pub prefer_reasoning: bool,

    /// Permission rules: a mode plus one `pattern = decision` table per tool.
    #[serde(default)]
    pub permissions: termide_agent_core::PermissionRules,

    /// Context compaction policy.
    #[serde(default)]
    pub compaction: termide_agent_core::CompactionPolicy,

    /// Fold each block in the transcript to a preview by default (the
    /// answer still shows in full); off shows everything expanded.
    #[serde(default = "agent_defaults::autofold")]
    pub autofold: bool,

    /// The web tools (`fetch`, `web_search`).
    #[serde(default)]
    pub web: WebSettings,
}

/// `[ai.web]`: how the agent's web tools reach the web.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSettings {
    /// `auto` (Chrome when found, else plain HTTP), `chrome` or `http`.
    /// Plain HTTP reads pages but cannot search.
    #[serde(default = "web_defaults::backend")]
    pub backend: String,
    /// Search engine: the name of a file under `ai/web/engines/`.
    #[serde(default = "web_defaults::engine")]
    pub engine: String,
    /// Browser executable; empty looks for Chrome, Chromium, Edge or Brave in
    /// the usual places.
    #[serde(default)]
    pub chrome_path: String,
    /// How the browser shows itself: `headless` (no window; a captcha brings
    /// one up for the user), `minimized` (a real window kept out of the way)
    /// or `visible` (a window on screen, to watch what the agent opens).
    #[serde(default = "web_defaults::display")]
    pub display: String,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            backend: web_defaults::backend(),
            engine: web_defaults::engine(),
            chrome_path: String::new(),
            display: web_defaults::display(),
        }
    }
}

/// Values `[ai.web] backend` accepts.
pub const WEB_BACKENDS: [&str; 3] = ["auto", "chrome", "http"];
/// Values `[ai.web] display` accepts.
pub const WEB_DISPLAYS: [&str; 3] = ["headless", "minimized", "visible"];

/// The search engines termide ships; the user may add more as files.
#[must_use]
pub fn builtin_web_engines() -> Vec<&'static str> {
    termide_agent_core::SEED_ENGINES
        .iter()
        .map(|(name, _)| *name)
        .collect()
}

mod web_defaults {
    pub fn backend() -> String {
        "auto".to_string()
    }
    pub fn engine() -> String {
        "duckduckgo".to_string()
    }
    pub fn display() -> String {
        "headless".to_string()
    }
}

impl AiSettings {
    /// The context window to start with: the configured cap when set, else the
    /// fallback used until the provider's real `max_model_len` is known.
    #[must_use]
    pub fn effective_context_window(&self) -> u64 {
        self.context_window_fallback
            .unwrap_or(DEFAULT_CONTEXT_WINDOW_FALLBACK)
    }

    /// The per-turn output bound to request, `None` when the setting is zero
    /// or negative and the model decides the length itself.
    #[must_use]
    pub fn output_limit(&self) -> Option<u64> {
        u64::try_from(self.max_tokens_per_turn)
            .ok()
            .filter(|&n| n > 0)
    }

    /// The permission mode new sessions start in, as configuration spells it
    /// (`ask`, `accept-edits`, `auto`, `plan`).
    #[must_use]
    pub fn permission_mode(&self) -> &'static str {
        self.permissions.mode.label()
    }

    /// Set the permission mode new sessions start in from its spelling;
    /// an unknown one is ignored.
    pub fn set_permission_mode(&mut self, label: &str) {
        if let Some(mode) = termide_agent_core::Mode::ALL
            .into_iter()
            .find(|mode| mode.label() == label)
        {
            self.permissions.mode = mode;
        }
    }
}

/// Every permission mode's spelling, in the order the UI offers them.
#[must_use]
pub fn permission_modes() -> Vec<&'static str> {
    termide_agent_core::Mode::ALL
        .into_iter()
        .map(termide_agent_core::Mode::label)
        .collect()
}

/// Whether an AI provider value names a CLI agent driven over ACP (Claude
/// Code, Codex) rather than a wire protocol the built-in loop speaks. Such a
/// provider brings its own endpoint, model and auth, so the endpoint/model/key
/// settings do not apply to it.
#[must_use]
pub fn is_cli_provider(provider: &str) -> bool {
    matches!(provider, "claude_code" | "codex")
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            provider: agent_defaults::provider(),
            base_url: agent_defaults::base_url(),
            model: String::new(),
            api_key_env: agent_defaults::api_key_env(),
            context_window_fallback: None,
            max_tokens_per_turn: agent_defaults::max_tokens(),
            prefer_reasoning: false,
            permissions: termide_agent_core::PermissionRules::default(),
            compaction: termide_agent_core::CompactionPolicy::default(),
            autofold: agent_defaults::autofold(),
            web: WebSettings::default(),
        }
    }
}

/// Syntax-highlighting settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HighlightSettings {
    /// User-defined keyword highlighters for file types that have no
    /// tree-sitter grammar. Each entry maps a set of extensions to a comment
    /// style plus keyword/type word lists.
    #[serde(default)]
    pub custom_languages: Vec<CustomLanguage>,
}

/// A user-defined keyword-based language for syntax highlighting.
///
/// Example (`config.toml`):
/// ```toml
/// [[highlight.custom_languages]]
/// name = "Alatyr"
/// extensions = ["al"]
/// line_comment = "##"
/// keywords = ["pub", "struct", "enum", "match", "if", "else", "and", "or", "not"]
/// types = ["u8", "i64", "usize", "ptr"]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomLanguage {
    /// Display name (shown in the editor status bar).
    #[serde(default)]
    pub name: String,
    /// File extensions (without the leading dot) this language applies to.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Line-comment lead-in, e.g. `"##"`, `"//"`, `"#"`.
    #[serde(default)]
    pub line_comment: Option<String>,
    /// Block-comment delimiters `["open", "close"]`, matched within one line.
    #[serde(default)]
    pub block_comment: Option<(String, String)>,
    /// Words coloured as keywords.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Words coloured as types.
    #[serde(default)]
    pub types: Vec<String>,
}

/// General application settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    /// Selected theme name
    #[serde(default = "default_theme_name")]
    pub theme: String,

    /// Interface language (en, de, es, fr, hi, pt, ru, th, zh, or auto)
    #[serde(default = "default_language")]
    pub language: String,

    /// Threshold width for auto-stacking panels (below this, panels stack vertically)
    #[serde(default = "default_auto_stack_threshold")]
    pub auto_stack_threshold: u16,

    /// Minimum panel width during resize operations
    #[serde(default = "default_min_panel_width")]
    pub min_panel_width: u16,

    /// How long a project's saved layout is kept, in days. The old
    /// `session_retention_days` spelling is still accepted so configs written
    /// before the rename keep working.
    #[serde(
        default = "default_project_retention_days",
        alias = "session_retention_days"
    )]
    pub project_retention_days: u32,

    /// Enable Vim mode globally (disabled by default)
    /// - In editor: NORMAL/INSERT/VISUAL modes, operators, motions
    /// - In list panels: j/k/g/G navigation
    #[serde(default = "default_vim_mode")]
    pub vim_mode: bool,

    /// Play bell sound when a file operation completes (enabled by default)
    #[serde(default = "default_bell_on_operation_complete")]
    pub bell_on_operation_complete: bool,

    /// Icon mode for panel titles (auto, emoji, unicode)
    #[serde(default)]
    pub icon_mode: IconMode,

    /// System resource monitor update interval in ms
    #[serde(default = "default_resource_monitor_interval")]
    pub resource_monitor_interval: u64,

    /// Ask the terminal to report **every** key as an escape code
    /// (Kitty `REPORT_ALL_KEYS_AS_ESCAPE_CODES`). Read at startup only.
    ///
    /// Honoured on macOS alone, where Option is a text-composition
    /// modifier: without the flag `Option+F` arrives as the composed
    /// glyph `ƒ` with no ALT bit and every `Alt+<letter>` default is
    /// unreachable. The cost is that dead-key and IME composition
    /// (`Option+E` `E` → `é`) no longer reaches termide, so users who
    /// need composed input can turn this off and rebind instead.
    #[serde(default = "default_true")]
    pub report_all_keys: bool,

    /// Start every session in a detachable host, so that closing the
    /// terminal leaves it running and `termide --attach` picks it back up
    /// without having to remember `--detached` at launch.
    ///
    /// Off by default: it changes what closing a terminal means. A session
    /// that outlives its window keeps its LSP servers, watchers and shells
    /// alive, which is the point when working over SSH and a surprise
    /// otherwise. Ignored when termide is launched with file arguments —
    /// `git commit` and friends wait for the editor to exit, and a detach
    /// would tell them the edit finished when it had not.
    ///
    /// Unix only; there is no session host on Windows.
    #[serde(default)]
    pub always_detachable: bool,

    /// Global keyboard shortcuts
    #[serde(default)]
    pub keybindings: GlobalKeybindings,
}

/// Editor settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    /// Tab size (number of spaces)
    #[serde(default = "default_tab_size")]
    pub tab_size: usize,

    /// Show git diff status colors on line numbers
    #[serde(default = "default_show_git_diff")]
    pub show_git_diff: bool,

    /// Enable word wrap in editor
    #[serde(default = "default_word_wrap")]
    pub word_wrap: bool,

    /// DEPRECATED: Use general.vim_mode instead.
    /// Kept for backward compatibility - will be migrated to general.vim_mode on load.
    #[serde(default, skip_serializing)]
    pub vim_mode: bool,

    /// Auto-indent new lines (inherit indentation from current line)
    #[serde(default = "default_true")]
    pub auto_indent: bool,

    /// Auto-close brackets and quotes
    #[serde(default = "default_true")]
    pub auto_close_brackets: bool,

    /// File size threshold in MB for disabling smart features
    #[serde(default = "default_large_file_threshold_mb")]
    pub large_file_threshold_mb: u64,

    /// Show inline git blame annotations
    #[serde(default = "default_true")]
    pub show_blame: bool,

    /// Editor keyboard shortcuts
    #[serde(default)]
    pub keybindings: EditorKeybindings,
}

/// File manager settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManagerSettings {
    /// Minimum width to display extended columns (size, time)
    #[serde(default = "default_extended_view_width")]
    pub extended_view_width: usize,

    /// Maximum file size in MB for content search (skip larger files)
    #[serde(default = "default_content_search_max_file_size_mb")]
    pub content_search_max_file_size_mb: u64,

    /// When `true`, the wide view computes and shows directory sizes in the
    /// Size column for local filesystems. Remote VFS is never walked.
    #[serde(default = "default_dir_size_in_wide_view")]
    pub dir_size_in_wide_view: bool,

    /// Per-directory time budget in milliseconds for that walk. A walk that
    /// exceeds this budget is reported with a dash marker. `0` disables the
    /// feature entirely.
    #[serde(default = "default_dir_size_budget_ms")]
    pub dir_size_budget_ms: u64,

    /// File manager keyboard shortcuts
    #[serde(default)]
    pub keybindings: FileManagerKeybindings,
}

/// Git status panel settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitStatusSettings {
    /// Git status panel keyboard shortcuts
    #[serde(default)]
    pub keybindings: GitStatusKeybindings,
}

/// Git diff panel settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitDiffSettings {
    /// Git diff panel keyboard shortcuts
    #[serde(default)]
    pub keybindings: GitDiffKeybindings,
}

/// Git log panel settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLogSettings {
    /// Git log panel keyboard shortcuts
    #[serde(default)]
    pub keybindings: GitLogKeybindings,
    /// Draw the commit graph with box-drawing pseudographics (`● │ ├ ╮ ╯`)
    /// computed from commit parents. When `false`, fall back to git's native
    /// ASCII `--graph` output.
    #[serde(default = "default_true")]
    pub unicode_graph: bool,
}

impl Default for GitLogSettings {
    fn default() -> Self {
        Self {
            keybindings: GitLogKeybindings::default(),
            unicode_graph: true,
        }
    }
}

/// Database viewer panel settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DatabaseSettings {
    /// Database viewer panel keyboard shortcuts
    #[serde(default)]
    pub keybindings: DatabaseKeybindings,
}

/// Where the viewers open a followed link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkOpen {
    /// Open inside the built-in viewer panel (text-mode browsing). Default.
    #[default]
    Panel,
    /// Open in the system's external browser.
    External,
}

/// File viewer panels settings (binary hex viewer, markdown/HTML preview).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViewerSettings {
    /// Viewer keyboard shortcuts
    #[serde(default)]
    pub keybindings: ViewerKeybindings,
    /// Where a followed page/link opens by default (the built-in panel, or the
    /// external browser). `O` always forces the external browser.
    #[serde(default)]
    pub open_links: LinkOpen,
    /// Where a followed link to an image opens by default (the built-in image
    /// preview, or the external viewer).
    #[serde(default)]
    pub open_images: LinkOpen,
}

/// Terminal panel settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TerminalSettings {
    /// Default shell path (None = auto-detect)
    #[serde(default)]
    pub default_shell: Option<String>,
    /// Terminal keyboard shortcuts
    #[serde(default)]
    pub keybindings: TerminalKeybindings,
}

/// Logging settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingSettings {
    /// Log file path (optional)
    #[serde(default)]
    pub file_path: Option<String>,

    /// Minimum log level (debug, info, warn, error)
    #[serde(default = "default_min_level")]
    pub min_level: String,
}

/// VFS (Virtual File System) settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfsSettings {
    /// Connection timeout in seconds (default: 60)
    #[serde(default = "default_vfs_connection_timeout")]
    pub connection_timeout_secs: u64,
}

impl Default for VfsSettings {
    fn default() -> Self {
        Self {
            connection_timeout_secs: default_vfs_connection_timeout(),
        }
    }
}

fn default_vfs_connection_timeout() -> u64 {
    60
}

/// LSP (Language Server Protocol) settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSettings {
    /// Enable LSP support
    #[serde(default = "default_lsp_enabled")]
    pub enabled: bool,

    /// Auto-trigger completion on typing
    #[serde(default = "default_lsp_auto_completion")]
    pub auto_completion: bool,

    /// Delay before triggering auto-completion (ms)
    #[serde(default = "default_lsp_completion_delay_ms")]
    pub completion_delay_ms: u64,

    /// Delay before showing hover documentation (ms)
    #[serde(default = "default_lsp_hover_delay_ms")]
    pub hover_delay_ms: u64,

    /// Per-language server configurations
    #[serde(default = "default_lsp_servers")]
    pub servers: std::collections::HashMap<String, LspServerSettings>,
}

/// Configuration for a specific LSP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerSettings {
    /// Command to start the server. Defaulted so a server entry that omits it
    /// deserializes to an (unusable, logged-at-spawn) empty command rather than
    /// failing the whole config document and discarding every other setting.
    #[serde(default)]
    pub command: String,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// File patterns to identify project root
    #[serde(default)]
    pub root_markers: Vec<String>,
}

// Default value functions for serde
fn default_theme_name() -> String {
    defaults::THEME_NAME.to_string()
}

mod agent_defaults {
    pub fn provider() -> String {
        // Unset by default: the user picks a provider (and a model) before the
        // AI panel will open, rather than silently defaulting to one.
        String::new()
    }
    pub fn base_url() -> String {
        "http://127.0.0.1:10000/v1".to_string()
    }
    pub fn api_key_env() -> String {
        // No key by default: local servers need none, and the right variable
        // depends on the provider, so the user names it when using a hosted one.
        String::new()
    }
    pub fn max_tokens() -> i64 {
        4_096
    }
    pub fn autofold() -> bool {
        true
    }
}

fn default_language() -> String {
    defaults::LANGUAGE.to_string()
}

fn default_auto_stack_threshold() -> u16 {
    defaults::AUTO_STACK_THRESHOLD
}

fn default_min_panel_width() -> u16 {
    defaults::MIN_PANEL_WIDTH
}

fn default_project_retention_days() -> u32 {
    defaults::PROJECT_RETENTION_DAYS
}

fn default_bell_on_operation_complete() -> bool {
    defaults::BELL_ON_OPERATION_COMPLETE
}

fn default_tab_size() -> usize {
    defaults::TAB_SIZE
}

fn default_show_git_diff() -> bool {
    defaults::SHOW_GIT_DIFF
}

fn default_word_wrap() -> bool {
    defaults::WORD_WRAP
}

fn default_vim_mode() -> bool {
    defaults::VIM_MODE
}

fn default_true() -> bool {
    true
}

fn default_large_file_threshold_mb() -> u64 {
    defaults::LARGE_FILE_THRESHOLD_MB
}

fn default_extended_view_width() -> usize {
    defaults::EXTENDED_VIEW_WIDTH
}

fn default_content_search_max_file_size_mb() -> u64 {
    defaults::CONTENT_SEARCH_MAX_FILE_SIZE_MB
}

fn default_dir_size_in_wide_view() -> bool {
    defaults::FM_DIR_SIZE_IN_WIDE_VIEW
}

fn default_dir_size_budget_ms() -> u64 {
    defaults::FM_DIR_SIZE_BUDGET_MS
}

fn default_min_level() -> String {
    defaults::MIN_LOG_LEVEL.to_string()
}

fn default_resource_monitor_interval() -> u64 {
    defaults::RESOURCE_MONITOR_INTERVAL
}

fn default_lsp_enabled() -> bool {
    defaults::LSP_ENABLED
}

fn default_lsp_auto_completion() -> bool {
    defaults::LSP_AUTO_COMPLETION
}

fn default_lsp_completion_delay_ms() -> u64 {
    defaults::LSP_COMPLETION_DELAY_MS
}

fn default_lsp_hover_delay_ms() -> u64 {
    defaults::LSP_HOVER_DELAY_MS
}

fn default_lsp_servers() -> std::collections::HashMap<String, LspServerSettings> {
    let mut servers = std::collections::HashMap::new();

    // Rust - rust-analyzer
    servers.insert(
        "rust".to_string(),
        LspServerSettings {
            command: "rust-analyzer".to_string(),
            args: vec![],
            root_markers: vec!["Cargo.toml".to_string()],
        },
    );

    // Python - pylsp or pyright
    servers.insert(
        "python".to_string(),
        LspServerSettings {
            command: "pylsp".to_string(),
            args: vec![],
            root_markers: vec![
                "pyproject.toml".to_string(),
                "setup.py".to_string(),
                "requirements.txt".to_string(),
            ],
        },
    );

    // TypeScript/JavaScript - typescript-language-server
    servers.insert(
        "typescript".to_string(),
        LspServerSettings {
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            root_markers: vec!["tsconfig.json".to_string(), "package.json".to_string()],
        },
    );

    servers.insert(
        "javascript".to_string(),
        LspServerSettings {
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            root_markers: vec!["package.json".to_string()],
        },
    );

    // Go - gopls
    servers.insert(
        "go".to_string(),
        LspServerSettings {
            command: "gopls".to_string(),
            args: vec![],
            root_markers: vec!["go.mod".to_string()],
        },
    );

    servers
}

/// Legacy flat config format for migration.
#[derive(Debug, Clone, Deserialize)]
pub struct LegacyConfig {
    #[serde(default = "default_theme_name")]
    pub theme: String,
    #[serde(default = "default_tab_size")]
    pub tab_size: usize,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub log_file_path: Option<String>,
    #[serde(default = "default_resource_monitor_interval")]
    pub resource_monitor_interval: u64,
    #[serde(default = "default_min_panel_width")]
    pub min_panel_width: u16,
    #[serde(default = "default_show_git_diff")]
    pub show_git_diff: bool,
    #[serde(default = "default_extended_view_width")]
    pub fm_extended_view_width: usize,
    // A pre-sections config file only ever spelled this the old way.
    #[serde(
        default = "default_project_retention_days",
        rename = "session_retention_days"
    )]
    pub project_retention_days: u32,
    #[serde(default = "default_word_wrap")]
    pub word_wrap: bool,
    #[serde(default = "default_min_level")]
    pub min_log_level: String,
    #[serde(default = "default_large_file_threshold_mb")]
    pub large_file_threshold_mb: u64,
}

impl From<LegacyConfig> for Config {
    fn from(legacy: LegacyConfig) -> Self {
        Self {
            general: GeneralSettings {
                theme: legacy.theme,
                language: legacy.language,
                auto_stack_threshold: legacy.min_panel_width, // migrate old field
                min_panel_width: default_min_panel_width(),
                project_retention_days: legacy.project_retention_days,
                vim_mode: default_vim_mode(),
                bell_on_operation_complete: default_bell_on_operation_complete(),
                icon_mode: IconMode::default(),
                resource_monitor_interval: legacy.resource_monitor_interval,
                report_all_keys: default_true(),
                always_detachable: false,
                keybindings: GlobalKeybindings::default(),
            },
            editor: EditorSettings {
                tab_size: legacy.tab_size,
                show_git_diff: legacy.show_git_diff,
                word_wrap: legacy.word_wrap,
                vim_mode: false, // deprecated, will be migrated
                auto_indent: true,
                auto_close_brackets: true,
                large_file_threshold_mb: legacy.large_file_threshold_mb,
                show_blame: true,
                keybindings: EditorKeybindings::default(),
            },
            file_manager: FileManagerSettings {
                extended_view_width: legacy.fm_extended_view_width,
                content_search_max_file_size_mb: defaults::CONTENT_SEARCH_MAX_FILE_SIZE_MB,
                dir_size_in_wide_view: defaults::FM_DIR_SIZE_IN_WIDE_VIEW,
                dir_size_budget_ms: defaults::FM_DIR_SIZE_BUDGET_MS,
                keybindings: FileManagerKeybindings::default(),
            },
            git_status: GitStatusSettings::default(),
            git_diff: GitDiffSettings::default(),
            git_log: GitLogSettings::default(),
            database: DatabaseSettings::default(),
            viewer: ViewerSettings::default(),
            terminal: TerminalSettings::default(),
            lsp: LspSettings::default(),
            logging: LoggingSettings {
                file_path: legacy.log_file_path,
                min_level: legacy.min_log_level,
            },
            vfs: VfsSettings::default(),
            highlight: HighlightSettings::default(),
            ai: AiSettings::default(),
        }
    }
}

// Default implementations
impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            theme: default_theme_name(),
            language: default_language(),
            auto_stack_threshold: default_auto_stack_threshold(),
            min_panel_width: default_min_panel_width(),
            project_retention_days: default_project_retention_days(),
            vim_mode: default_vim_mode(),
            bell_on_operation_complete: default_bell_on_operation_complete(),
            icon_mode: IconMode::default(),
            resource_monitor_interval: default_resource_monitor_interval(),
            report_all_keys: default_true(),
            always_detachable: false,
            keybindings: GlobalKeybindings::default(),
        }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: default_tab_size(),
            show_git_diff: default_show_git_diff(),
            word_wrap: default_word_wrap(),
            vim_mode: false, // deprecated, use general.vim_mode
            auto_indent: true,
            auto_close_brackets: true,
            large_file_threshold_mb: default_large_file_threshold_mb(),
            show_blame: true,
            keybindings: EditorKeybindings::default(),
        }
    }
}

impl Default for FileManagerSettings {
    fn default() -> Self {
        Self {
            extended_view_width: default_extended_view_width(),
            content_search_max_file_size_mb: default_content_search_max_file_size_mb(),
            dir_size_in_wide_view: default_dir_size_in_wide_view(),
            dir_size_budget_ms: default_dir_size_budget_ms(),
            keybindings: FileManagerKeybindings::default(),
        }
    }
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            file_path: None,
            min_level: default_min_level(),
        }
    }
}

impl Default for LspSettings {
    fn default() -> Self {
        Self {
            enabled: default_lsp_enabled(),
            auto_completion: default_lsp_auto_completion(),
            completion_delay_ms: default_lsp_completion_delay_ms(),
            hover_delay_ms: default_lsp_hover_delay_ms(),
            servers: default_lsp_servers(),
        }
    }
}

impl Config {
    /// Fill all None keybinding values with their defaults.
    ///
    /// This ensures that when serializing to TOML, all keybindings
    /// are written with their values (either user-configured or defaults).
    /// Also migrates deprecated settings (e.g., editor.vim_mode -> general.vim_mode).
    pub fn normalize(&mut self) {
        // Migrate deprecated editor.vim_mode to general.vim_mode
        // If editor.vim_mode is true and general.vim_mode is false (default),
        // it means user has old config with [editor] vim_mode = true
        if self.editor.vim_mode && !self.general.vim_mode {
            self.general.vim_mode = true;
        }
        // Clear deprecated field after migration
        self.editor.vim_mode = false;

        self.general.keybindings.with_defaults();
        self.editor.keybindings.with_defaults();
        self.file_manager.keybindings.with_defaults();
        self.git_status.keybindings.with_defaults();
        self.git_diff.keybindings.with_defaults();
        self.git_log.keybindings.with_defaults();
        self.database.keybindings.with_defaults();
        self.viewer.keybindings.with_defaults();
        self.terminal.keybindings.with_defaults();
    }
}

#[cfg(test)]
mod ai_settings_tests {
    use super::*;

    #[test]
    fn a_non_positive_output_limit_means_none() {
        let mut settings = AiSettings::default();
        assert_eq!(settings.output_limit(), Some(4096));
        settings.max_tokens_per_turn = 0;
        assert_eq!(settings.output_limit(), None);
        settings.max_tokens_per_turn = -1;
        assert_eq!(settings.output_limit(), None);
        // A negative number is valid in the config file.
        let parsed: AiSettings = toml::from_str("max_tokens_per_turn = -1").unwrap();
        assert_eq!(parsed.output_limit(), None);
    }
}

#[cfg(test)]
mod keybinding_default_tests {
    use super::*;
    use crate::KeyBinding;

    /// A config saved by an older version materialises the whole
    /// `[general.keybindings]` table, so every binding that existed then is
    /// present and any binding added later is absent. `normalize()` has to
    /// fill the new one in, or the feature it belongs to is unreachable for
    /// every existing user while working fine on a fresh install.
    /// An older config carries the whole binding table verbatim, including
    /// defaults this version has replaced. Those frozen copies must give way,
    /// or the new bindings are unreachable for exactly the users who have been
    /// running termide the longest — `Alt+D` would still switch panel groups
    /// rather than detach.
    #[test]
    fn superseded_defaults_give_way_to_the_new_ones() {
        let toml = r#"
[general.keybindings]
next_group = ["Alt+Right", "Alt+D"]
prev_group = ["Alt+Left", "Alt+A"]
prev_panel = ["Alt+Up", "Alt+W"]
next_panel = ["Alt+Down", "Alt+S"]
close_panel = ["Alt+X", "F10"]
"#;
        let mut config: Config = toml::from_str(toml).expect("config parses");
        config.normalize();

        let kb = &config.general.keybindings;
        assert_eq!(
            kb.next_group,
            Some(KeyBinding::Single("Alt+Right".to_string()))
        );
        assert_eq!(
            kb.prev_panel,
            Some(KeyBinding::Single("Alt+Up".to_string()))
        );
        assert_eq!(
            kb.detach_instance,
            Some(KeyBinding::Single("Alt+D".to_string())),
            "the freed letter must now reach detach"
        );
        assert_eq!(
            kb.close_panel,
            Some(KeyBinding::Multiple(vec![
                "Alt+W".to_string(),
                "Alt+X".to_string(),
                "F10".to_string()
            ]))
        );
    }

    /// A binding the user chose themselves is never rewritten, even when it
    /// mentions the same keys.
    #[test]
    fn a_deliberate_binding_survives_the_migration() {
        let toml = r#"
[general.keybindings]
next_group = ["Alt+D"]
prev_panel = "Alt+W"
"#;
        let mut config: Config = toml::from_str(toml).expect("config parses");
        config.normalize();

        let kb = &config.general.keybindings;
        assert_eq!(
            kb.next_group,
            Some(KeyBinding::Multiple(vec!["Alt+D".to_string()])),
            "only a verbatim copy of the old default is dropped"
        );
        assert_eq!(kb.prev_panel, Some(KeyBinding::Single("Alt+W".to_string())));
    }

    #[test]
    fn a_binding_added_later_is_filled_into_an_older_saved_config() {
        let toml = r#"
[general]
language = "ru"

[general.keybindings]
quit = "Alt+Q"
new_terminal = "Alt+T"
next_group = ["Alt+Right", "Alt+D"]
"#;
        let mut config: Config = toml::from_str(toml).expect("config parses");
        assert!(
            config.general.keybindings.detach_instance.is_none(),
            "precondition: the saved file has no detach_instance"
        );

        config.normalize();

        let binding = config
            .general
            .keybindings
            .detach_instance
            .expect("normalize fills in the new binding");
        assert_eq!(binding, KeyBinding::Single("Alt+D".to_string()));

        // Bindings the file did set must survive untouched.
        assert_eq!(
            config.general.keybindings.quit,
            Some(KeyBinding::Single("Alt+Q".to_string()))
        );
    }
}
