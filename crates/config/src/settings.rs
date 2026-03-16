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
    EditorKeybindings, FileManagerKeybindings, GitStatusKeybindings, GlobalKeybindings,
    TerminalKeybindings,
};

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

    /// Session retention period in days
    #[serde(default = "default_session_retention_days")]
    pub session_retention_days: u32,

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
    /// Command to start the server
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

fn default_language() -> String {
    defaults::LANGUAGE.to_string()
}

fn default_auto_stack_threshold() -> u16 {
    defaults::AUTO_STACK_THRESHOLD
}

fn default_min_panel_width() -> u16 {
    defaults::MIN_PANEL_WIDTH
}

fn default_session_retention_days() -> u32 {
    defaults::SESSION_RETENTION_DAYS
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
    #[serde(default = "default_session_retention_days")]
    pub session_retention_days: u32,
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
                session_retention_days: legacy.session_retention_days,
                vim_mode: default_vim_mode(),
                bell_on_operation_complete: default_bell_on_operation_complete(),
                icon_mode: IconMode::default(),
                resource_monitor_interval: legacy.resource_monitor_interval,
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
                keybindings: EditorKeybindings::default(),
            },
            file_manager: FileManagerSettings {
                extended_view_width: legacy.fm_extended_view_width,
                content_search_max_file_size_mb: defaults::CONTENT_SEARCH_MAX_FILE_SIZE_MB,
                keybindings: FileManagerKeybindings::default(),
            },
            git_status: GitStatusSettings::default(),
            terminal: TerminalSettings::default(),
            lsp: LspSettings::default(),
            logging: LoggingSettings {
                file_path: legacy.log_file_path,
                min_level: legacy.min_log_level,
            },
            vfs: VfsSettings::default(),
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
            session_retention_days: default_session_retention_days(),
            vim_mode: default_vim_mode(),
            bell_on_operation_complete: default_bell_on_operation_complete(),
            icon_mode: IconMode::default(),
            resource_monitor_interval: default_resource_monitor_interval(),
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
            keybindings: EditorKeybindings::default(),
        }
    }
}

impl Default for FileManagerSettings {
    fn default() -> Self {
        Self {
            extended_view_width: default_extended_view_width(),
            content_search_max_file_size_mb: default_content_search_max_file_size_mb(),
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
        self.terminal.keybindings.with_defaults();
    }
}
