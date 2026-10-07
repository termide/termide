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

    /// Password vault settings
    #[serde(default)]
    pub vault: VaultSettings,

    /// Syntax-highlighting settings (custom keyword languages)
    #[serde(default)]
    pub highlight: HighlightSettings,

    /// AI settings (model access and the coding agent panel).
    #[serde(default)]
    pub ai: AiSettings,
}

/// AI settings: the connections to models and what the agent may do.
///
/// A connection (`[ai.connections.<name>]`) is one endpoint and model, or a
/// CLI agent; everything else here applies whichever one a session runs on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSettings {
    /// The connection new sessions start on. Empty, or naming none that
    /// exists, falls back to the first by name.
    #[serde(default)]
    pub connection: String,

    /// The connections, by name.
    #[serde(default)]
    pub connections: std::collections::BTreeMap<String, Connection>,

    /// Upper bound on the model's output tokens per turn (one response). Zero
    /// or a negative number sets no bound and leaves the length to the model.
    #[serde(default = "agent_defaults::max_tokens")]
    pub max_tokens_per_turn: i64,

    /// The reasoning level new sessions ask for (`off` through `max`); a
    /// model that lacks it gets the nearest one it has. The older
    /// `prefer_reasoning = true | false` reads as `high` or `off`.
    #[serde(
        default = "agent_defaults::reasoning",
        alias = "prefer_reasoning",
        deserialize_with = "reasoning_level"
    )]
    pub reasoning: termide_agent_core::ThinkingLevel,

    /// Permission rules: a mode (`configured` unless the file names another)
    /// plus one `pattern = decision` table per tool.
    #[serde(default)]
    pub permissions: termide_agent_core::PermissionRules,

    /// `[ai.auto_reviewer]`: the model that reviews calls in `auto` mode.
    /// Left empty, the model the session runs on reviews.
    #[serde(default)]
    pub auto_reviewer: SideModel,

    /// Context compaction policy.
    #[serde(default)]
    pub compaction: termide_agent_core::CompactionPolicy,

    /// When the transcript folds reasoning and tool calls to their one-line
    /// headline (the answer always shows in full).
    #[serde(default)]
    pub fold_blocks: FoldBlocks,

    /// The web tools (`fetch`, `web_search`).
    #[serde(default)]
    pub web: WebSettings,

    /// The `recall` tool: its sources' time limits and its solver.
    #[serde(default)]
    pub recall: RecallSettings,

    /// Ring the terminal bell when an agent panel waits for the user out of
    /// sight: a permission or question card, or a long run that finished.
    #[serde(default = "agent_defaults::bell_on_attention")]
    pub bell_on_attention: bool,
}

/// One connection to a model: the wire protocol or CLI agent, where it
/// listens, the key and the model. The API key is read from `api_key_env`
/// rather than stored, so the config file never holds a secret.
///
/// Only what a connection sets is written: a new one is a table the
/// defaults do not have, so the diff-against-defaults save would otherwise
/// write every field, the ones its provider ignores included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    /// `openai_compatible` (omlx, llama.cpp, OpenAI, OpenRouter and most
    /// gateways), `anthropic_compatible` (the Messages API), or a CLI agent
    /// driven over ACP: `claude_code`, `codex`, `gemini_cli`.
    #[serde(default = "agent_defaults::provider")]
    pub provider: String,
    /// Base URL including the API prefix, e.g. `http://127.0.0.1:10000/v1`.
    /// For `anthropic_compatible` it is left at the default unless a gateway
    /// is used.
    #[serde(
        default = "agent_defaults::base_url",
        skip_serializing_if = "agent_defaults::is_base_url"
    )]
    pub base_url: String,
    /// Model id as the endpoint expects it; for a CLI agent, the model to
    /// pre-select on its own login.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Environment variable holding the API key; empty for local servers.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key_env: String,
    /// Fallback context window in tokens, used only when the provider does
    /// not report a model's window. Unset falls back to
    /// [`DEFAULT_CONTEXT_WINDOW_FALLBACK`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_fallback: Option<u64>,
    /// Ask an `openai_compatible` server for its prompt-processing progress
    /// (llama.cpp's `return_progress`), for a live prefill bar. Off by
    /// default: servers that do not know the field may reject the request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prefill_progress: bool,
    /// The field an `openai_compatible` server takes the reasoning level in.
    #[serde(default, skip_serializing_if = "ReasoningParam::is_auto")]
    pub reasoning_param: ReasoningParam,
    /// The connection, by name, the agents this one delegates to with
    /// `task` run on; empty runs them on this one. A subagent runs the
    /// built-in loop, so it needs a model connection, not a CLI agent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subagents: String,
    /// How many requests the connection serves at once across the termide
    /// process — the panels' agents, their subagents and side calls — the
    /// rest waiting their turn; 0 sets no limit. A local server usually runs
    /// one at a time (or llama.cpp's `--parallel` many).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_concurrent_requests: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `reasoning_param` of an `openai_compatible` connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningParam {
    /// `reasoning_effort` for the hosted APIs known to take it (OpenAI,
    /// OpenRouter, Gemini), `enable_thinking` for a server on this machine
    /// or the local network, nothing elsewhere.
    #[default]
    Auto,
    /// `reasoning_effort`, the OpenAI field.
    ReasoningEffort,
    /// `chat_template_kwargs.enable_thinking`, the on/off switch of the chat
    /// templates of Qwen3, GLM and DeepSeek on vLLM or llama.cpp.
    EnableThinking,
    /// Nothing: the server decides.
    None,
}

impl ReasoningParam {
    pub const ALL: [Self; 4] = [
        Self::Auto,
        Self::ReasoningEffort,
        Self::EnableThinking,
        Self::None,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ReasoningEffort => "reasoning_effort",
            Self::EnableThinking => "enable_thinking",
            Self::None => "none",
        }
    }

    #[must_use]
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

/// A reasoning level, or the on/off switch it replaced.
fn reasoning_level<'de, D>(deserializer: D) -> Result<termide_agent_core::ThinkingLevel, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use termide_agent_core::ThinkingLevel;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum LevelOrSwitch {
        Switch(bool),
        Level(ThinkingLevel),
    }
    Ok(match LevelOrSwitch::deserialize(deserializer)? {
        LevelOrSwitch::Switch(true) => ThinkingLevel::High,
        LevelOrSwitch::Switch(false) => ThinkingLevel::Off,
        LevelOrSwitch::Level(level) => level,
    })
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            provider: agent_defaults::provider(),
            base_url: agent_defaults::base_url(),
            model: String::new(),
            api_key_env: String::new(),
            context_window_fallback: None,
            prefill_progress: false,
            reasoning_param: ReasoningParam::Auto,
            subagents: String::new(),
            max_concurrent_requests: 0,
        }
    }
}

impl Connection {
    /// The context window to start with, until the provider reports one.
    #[must_use]
    pub fn effective_context_window(&self) -> u64 {
        self.context_window_fallback
            .unwrap_or(DEFAULT_CONTEXT_WINDOW_FALLBACK)
    }

    /// Whether it drives a CLI agent over ACP rather than an endpoint.
    #[must_use]
    pub fn is_cli(&self) -> bool {
        is_cli_provider(&self.provider)
    }

    /// Whether the agents a session delegates to can run on it: termide's
    /// own loop on a model connection, or a copy of Claude Code on termide's
    /// tools. Codex and Gemini CLI keep tools of their own, so termide does
    /// not run its subagents on them.
    #[must_use]
    pub fn runs_subagents(&self) -> bool {
        !self.is_cli() || self.provider == "claude_code"
    }
}

/// `[ai] fold_blocks`: when reasoning and tool calls fold to one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FoldBlocks {
    /// Folded from the start, while they still run.
    #[default]
    Immediately,
    /// In full while they run, folded once they finish.
    OnFinish,
    /// Never folded.
    Never,
}

impl FoldBlocks {
    /// Every choice, in the order the settings modal offers them.
    pub const ALL: [FoldBlocks; 3] = [
        FoldBlocks::Immediately,
        FoldBlocks::OnFinish,
        FoldBlocks::Never,
    ];

    /// The spelling configuration uses.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            FoldBlocks::Immediately => "immediately",
            FoldBlocks::OnFinish => "on-finish",
            FoldBlocks::Never => "never",
        }
    }
}

/// A model for a side call — the `auto` reviewer, the recall solver: a
/// connection, a model or a CLI agent's subscription, and the model of it to
/// use. An empty connection means the model the session runs on; an empty
/// model, the connection's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideModel {
    #[serde(default)]
    pub connection: String,
    #[serde(default)]
    pub model: String,
}

/// `[ai.recall]`: how long each source of a `recall` search may take, and whether it
/// answers from its results with one model call before handing them back,
/// and with which model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallSettings {
    /// Seconds each source may search, 0 for no limit; the sources search
    /// side by side, and what one found by its limit is returned, with a
    /// note that it stopped.
    #[serde(default = "recall_defaults::timeout_secs")]
    pub sessions_timeout_secs: u64,
    #[serde(default = "recall_defaults::timeout_secs")]
    pub git_timeout_secs: u64,
    #[serde(default = "recall_defaults::timeout_secs")]
    pub files_timeout_secs: u64,
    /// Answer from the results instead of returning them as found.
    #[serde(default)]
    pub solver: bool,
    /// The connection whose model answers. Empty answers with the model the
    /// session runs on.
    #[serde(default)]
    pub connection: String,
    /// The model of that connection to answer with; empty takes the
    /// connection's own.
    #[serde(default)]
    pub model: String,
}

impl Default for RecallSettings {
    fn default() -> Self {
        Self {
            sessions_timeout_secs: recall_defaults::timeout_secs(),
            git_timeout_secs: recall_defaults::timeout_secs(),
            files_timeout_secs: recall_defaults::timeout_secs(),
            solver: false,
            connection: String::new(),
            model: String::new(),
        }
    }
}

mod recall_defaults {
    pub fn timeout_secs() -> u64 {
        60
    }
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
    /// The name of the connection new sessions start on: `connection` when it
    /// exists, else the first by name; `None` with no connections at all.
    #[must_use]
    pub fn default_connection(&self) -> Option<&str> {
        if self.connections.contains_key(&self.connection) {
            return Some(self.connection.as_str());
        }
        self.connections.keys().next().map(String::as_str)
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
    /// (`ask`, `plan`, `edit`, `configured`, `all`).
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
/// Code, Codex, Gemini CLI) rather than a wire protocol the built-in loop speaks. Such a
/// provider brings its own endpoint, model and auth, so the endpoint/model/key
/// settings do not apply to it.
#[must_use]
pub fn is_cli_provider(provider: &str) -> bool {
    matches!(provider, "claude_code" | "codex" | "gemini_cli")
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            connection: String::new(),
            connections: std::collections::BTreeMap::new(),
            max_tokens_per_turn: agent_defaults::max_tokens(),
            reasoning: agent_defaults::reasoning(),
            permissions: termide_agent_core::PermissionRules::default(),
            auto_reviewer: SideModel::default(),
            compaction: termide_agent_core::CompactionPolicy::default(),
            fold_blocks: FoldBlocks::default(),
            web: WebSettings::default(),
            recall: RecallSettings::default(),
            bell_on_attention: agent_defaults::bell_on_attention(),
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

    /// Start every instance in a detachable host, so that closing the
    /// terminal leaves it running and `termide --attach` picks it back up
    /// without having to remember `--detached` at launch.
    ///
    /// Off by default: it changes what closing a terminal means. An instance
    /// that outlives its window keeps its LSP servers, watchers and shells
    /// alive, which is the point when working over SSH and a surprise
    /// otherwise. Ignored when termide is launched with file arguments —
    /// `git commit` and friends wait for the editor to exit, and a detach
    /// would tell them the edit finished when it had not.
    ///
    /// Unix only; there is no instance host on Windows.
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

/// Password vault settings (`[vault]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultSettings {
    /// Minutes without use after which the unlocked vault locks itself;
    /// 0 keeps it unlocked until termide exits (default: 15).
    #[serde(default = "default_vault_lock_after_mins")]
    pub lock_after_mins: u64,
}

impl Default for VaultSettings {
    fn default() -> Self {
        Self {
            lock_after_mins: default_vault_lock_after_mins(),
        }
    }
}

fn default_vault_lock_after_mins() -> u64 {
    15
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

    /// Per-language server configurations: the built-ins, with each user
    /// entry laid over the built-in of the same language or added beside them.
    #[serde(default = "default_lsp_servers", deserialize_with = "lsp_servers")]
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
        "openai_compatible".to_string()
    }
    pub fn base_url() -> String {
        "http://127.0.0.1:10000/v1".to_string()
    }
    pub fn is_base_url(value: &str) -> bool {
        value == base_url()
    }
    pub fn max_tokens() -> i64 {
        // No bound: the model decides how long to answer.
        0
    }
    pub fn reasoning() -> termide_agent_core::ThinkingLevel {
        termide_agent_core::ThinkingLevel::High
    }
    pub fn bell_on_attention() -> bool {
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

    // TypeScript/JavaScript - typescript-language-server. The JSX variants
    // are languages of their own to the server, so they get their own keys.
    for lang in ["typescript", "typescriptreact"] {
        servers.insert(
            lang.to_string(),
            LspServerSettings {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
                root_markers: vec!["tsconfig.json".to_string(), "package.json".to_string()],
            },
        );
    }

    for lang in ["javascript", "javascriptreact"] {
        servers.insert(
            lang.to_string(),
            LspServerSettings {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
                root_markers: vec!["package.json".to_string()],
            },
        );
    }

    // Go - gopls
    servers.insert(
        "go".to_string(),
        LspServerSettings {
            command: "gopls".to_string(),
            args: vec![],
            root_markers: vec!["go.mod".to_string()],
        },
    );

    // PHP - PHPantom
    servers.insert(
        "php".to_string(),
        LspServerSettings {
            command: "phpantom_lsp".to_string(),
            args: vec![],
            root_markers: vec!["composer.json".to_string()],
        },
    );

    // Terraform - terraform-ls
    for lang in ["terraform", "terraform-vars"] {
        servers.insert(
            lang.to_string(),
            LspServerSettings {
                command: "terraform-ls".to_string(),
                args: vec!["serve".to_string()],
                root_markers: vec![".terraform.lock.hcl".to_string(), ".terraform".to_string()],
            },
        );
    }

    // Dockerfile/Compose - docker-language-server
    for lang in ["dockerfile", "dockercompose"] {
        servers.insert(
            lang.to_string(),
            LspServerSettings {
                command: "docker-language-server".to_string(),
                args: vec!["start".to_string(), "--stdio".to_string()],
                root_markers: vec![
                    "compose.yaml".to_string(),
                    "compose.yml".to_string(),
                    "docker-compose.yaml".to_string(),
                    "docker-compose.yml".to_string(),
                ],
            },
        );
    }

    servers
}

/// `[lsp.servers]` as written, laid over [`default_lsp_servers`] the way the
/// layered loader overlays files: an entry for a built-in language changes
/// only the fields it sets, any other entry is added. Without this a single
/// entry would replace the whole built-in table whenever a config is parsed
/// directly (`--config`, saving the config file from the editor).
fn lsp_servers<'de, D>(
    deserializer: D,
) -> Result<std::collections::HashMap<String, LspServerSettings>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let entries = std::collections::HashMap::<String, toml::Value>::deserialize(deserializer)?;
    let mut servers = default_lsp_servers();
    for (lang, entry) in entries {
        let merged = match servers.remove(&lang) {
            Some(builtin) => {
                let mut value = toml::Value::try_from(builtin).map_err(D::Error::custom)?;
                crate::diff::merge_partial(&mut value, &entry);
                value
            }
            None => entry,
        };
        let server = merged
            .try_into()
            .map_err(|e: toml::de::Error| D::Error::custom(format!("{lang}: {}", e.message())))?;
        servers.insert(lang, server);
    }
    Ok(servers)
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
            vault: VaultSettings::default(),
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
    fn recall_settings_keep_their_defaults_where_left_out() {
        let parsed: AiSettings = toml::from_str(
            r#"
            [recall]
            git_timeout_secs = 0
            solver = true
            "#,
        )
        .unwrap();
        assert_eq!(
            parsed.recall,
            RecallSettings {
                git_timeout_secs: 0,
                solver: true,
                ..RecallSettings::default()
            }
        );
        assert_eq!(parsed.recall.files_timeout_secs, 60);
        assert_eq!(AiSettings::default().recall, RecallSettings::default());
    }

    #[test]
    fn new_sessions_start_on_the_named_connection_or_the_first() {
        let mut parsed: AiSettings = toml::from_str(
            r#"
            connection = "local"
            [connections.local]
            model = "local-model"
            [connections.cloud]
            provider = "anthropic_compatible"
            model = "claude-x"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.default_connection(), Some("local"));
        let local = &parsed.connections["local"];
        // What a connection leaves out takes the field's own default.
        assert_eq!(local.provider, "openai_compatible");
        assert_eq!(local.base_url, agent_defaults::base_url());
        parsed.connection = "gone".into();
        assert_eq!(parsed.default_connection(), Some("cloud"));
        assert_eq!(AiSettings::default().default_connection(), None);
    }

    #[test]
    fn a_connection_writes_only_what_it_sets() {
        let cli = Connection {
            provider: "claude_code".into(),
            ..Connection::default()
        };
        assert_eq!(
            toml::to_string(&cli).unwrap().trim(),
            r#"provider = "claude_code""#
        );
        let local = Connection {
            model: "qwen".into(),
            ..Connection::default()
        };
        let text = toml::to_string(&local).unwrap();
        assert_eq!(
            text.trim(),
            "provider = \"openai_compatible\"\nmodel = \"qwen\""
        );
        // What it leaves out reads back as the default.
        assert_eq!(toml::from_str::<Connection>(&text).unwrap(), local);
    }

    #[test]
    fn new_sessions_reason_and_start_in_auto() {
        let defaults = AiSettings::default();
        assert_eq!(defaults.reasoning, termide_agent_core::ThinkingLevel::High);
        assert!(defaults.bell_on_attention);
        assert_eq!(defaults.permissions.mode, termide_agent_core::Mode::Auto);
        // A file that only adds a rule keeps the mode.
        let parsed: AiSettings =
            toml::from_str("[permissions.bash]\n\"ls *\" = \"allow\"\n").unwrap();
        assert_eq!(parsed.permissions.mode, termide_agent_core::Mode::Auto);
        assert_eq!(
            parsed.permissions.evaluate("bash", "ls -la"),
            Some(termide_agent_core::Decision::Allow)
        );
        let parsed: AiSettings =
            toml::from_str("prefer_reasoning = false\n[permissions]\nmode = \"auto\"\n").unwrap();
        assert_eq!(parsed.permissions.mode, termide_agent_core::Mode::Auto);
        assert_eq!(parsed.auto_reviewer, SideModel::default());

        assert_eq!(parsed.reasoning, termide_agent_core::ThinkingLevel::Off);
        let reviewer: AiSettings =
            toml::from_str("[auto_reviewer]\nconnection = \"claude\"\nmodel = \"haiku\"\n")
                .unwrap();
        assert_eq!(
            reviewer.auto_reviewer,
            SideModel {
                connection: "claude".into(),
                model: "haiku".into()
            }
        );
        let parsed: AiSettings = toml::from_str("prefer_reasoning = true\n").unwrap();
        assert_eq!(parsed.reasoning, termide_agent_core::ThinkingLevel::High);
        let parsed: AiSettings = toml::from_str("reasoning = \"xhigh\"\n").unwrap();
        assert_eq!(parsed.reasoning, termide_agent_core::ThinkingLevel::XHigh);
        assert!(toml::from_str::<AiSettings>("reasoning = \"extreme\"\n").is_err());
    }

    #[test]
    fn a_connection_names_its_reasoning_param_only_when_set() {
        let mut local = Connection::default();
        assert!(!toml::to_string(&local).unwrap().contains("reasoning_param"));
        local.reasoning_param = ReasoningParam::EnableThinking;
        let text = toml::to_string(&local).unwrap();
        assert!(
            text.contains("reasoning_param = \"enable_thinking\""),
            "{text}"
        );
        assert_eq!(toml::from_str::<Connection>(&text).unwrap(), local);
    }

    #[test]
    fn a_connection_names_its_subagents_connection_only_when_set() {
        let mut claude = Connection {
            provider: "claude_code".into(),
            ..Connection::default()
        };
        assert!(!toml::to_string(&claude).unwrap().contains("subagents"));
        claude.subagents = "local".into();
        let text = toml::to_string(&claude).unwrap();
        assert!(text.contains("subagents = \"local\""), "{text}");
        assert_eq!(toml::from_str::<Connection>(&text).unwrap(), claude);
    }

    #[test]
    fn a_connection_names_its_request_limit_only_when_set() {
        let mut local = Connection::default();
        assert!(!toml::to_string(&local)
            .unwrap()
            .contains("max_concurrent_requests"));
        local.max_concurrent_requests = 1;
        let text = toml::to_string(&local).unwrap();
        assert!(text.contains("max_concurrent_requests = 1"), "{text}");
        assert_eq!(toml::from_str::<Connection>(&text).unwrap(), local);
    }

    #[test]
    fn a_non_positive_output_limit_means_none() {
        let mut settings = AiSettings::default();
        // No bound by default: the model decides.
        assert_eq!(settings.output_limit(), None);
        settings.max_tokens_per_turn = 4096;
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

#[cfg(test)]
mod lsp_default_tests {
    use super::*;

    #[test]
    fn built_in_servers_cover_php_terraform_and_docker() {
        let servers = default_lsp_servers();
        let expected: [(&str, &str, &[&str], &[&str]); 5] = [
            ("php", "phpantom_lsp", &[], &["composer.json"]),
            (
                "terraform",
                "terraform-ls",
                &["serve"],
                &[".terraform.lock.hcl", ".terraform"],
            ),
            (
                "terraform-vars",
                "terraform-ls",
                &["serve"],
                &[".terraform.lock.hcl", ".terraform"],
            ),
            (
                "dockerfile",
                "docker-language-server",
                &["start", "--stdio"],
                &[
                    "compose.yaml",
                    "compose.yml",
                    "docker-compose.yaml",
                    "docker-compose.yml",
                ],
            ),
            (
                "dockercompose",
                "docker-language-server",
                &["start", "--stdio"],
                &[
                    "compose.yaml",
                    "compose.yml",
                    "docker-compose.yaml",
                    "docker-compose.yml",
                ],
            ),
        ];
        for (lang, command, args, root_markers) in expected {
            let server = servers.get(lang).unwrap_or_else(|| panic!("{lang}"));
            assert_eq!(server.command, command, "{lang}");
            assert_eq!(server.args, args, "{lang}");
            // The root the server runs in decides what it can resolve, so the
            // markers are as much of the definition as the command is.
            assert_eq!(server.root_markers, root_markers, "{lang}");
        }
    }
}
