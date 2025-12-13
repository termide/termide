//! Configuration structures for termide settings.

use serde::{Deserialize, Serialize};

use crate::defaults;

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

    /// Logging settings
    #[serde(default)]
    pub logging: LoggingSettings,
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

    /// Minimum panel width in characters
    #[serde(default = "default_min_panel_width")]
    pub min_panel_width: u16,

    /// Session retention period in days
    #[serde(default = "default_session_retention_days")]
    pub session_retention_days: u32,
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

    /// File size threshold in MB for disabling smart features
    #[serde(default = "default_large_file_threshold_mb")]
    pub large_file_threshold_mb: u64,
}

/// File manager settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManagerSettings {
    /// Minimum width to display extended columns (size, time)
    #[serde(default = "default_extended_view_width")]
    pub extended_view_width: usize,
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

    /// System resource monitor update interval in ms
    #[serde(default = "default_resource_monitor_interval")]
    pub resource_monitor_interval: u64,
}

// Default value functions for serde
fn default_theme_name() -> String {
    defaults::THEME_NAME.to_string()
}

fn default_language() -> String {
    defaults::LANGUAGE.to_string()
}

fn default_min_panel_width() -> u16 {
    defaults::MIN_PANEL_WIDTH
}

fn default_session_retention_days() -> u32 {
    defaults::SESSION_RETENTION_DAYS
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

fn default_large_file_threshold_mb() -> u64 {
    defaults::LARGE_FILE_THRESHOLD_MB
}

fn default_extended_view_width() -> usize {
    defaults::EXTENDED_VIEW_WIDTH
}

fn default_min_level() -> String {
    defaults::MIN_LOG_LEVEL.to_string()
}

fn default_resource_monitor_interval() -> u64 {
    defaults::RESOURCE_MONITOR_INTERVAL
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
                min_panel_width: legacy.min_panel_width,
                session_retention_days: legacy.session_retention_days,
            },
            editor: EditorSettings {
                tab_size: legacy.tab_size,
                show_git_diff: legacy.show_git_diff,
                word_wrap: legacy.word_wrap,
                large_file_threshold_mb: legacy.large_file_threshold_mb,
            },
            file_manager: FileManagerSettings {
                extended_view_width: legacy.fm_extended_view_width,
            },
            logging: LoggingSettings {
                file_path: legacy.log_file_path,
                min_level: legacy.min_log_level,
                resource_monitor_interval: legacy.resource_monitor_interval,
            },
        }
    }
}

// Default implementations
impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            theme: default_theme_name(),
            language: default_language(),
            min_panel_width: default_min_panel_width(),
            session_retention_days: default_session_retention_days(),
        }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: default_tab_size(),
            show_git_diff: default_show_git_diff(),
            word_wrap: default_word_wrap(),
            large_file_threshold_mb: default_large_file_threshold_mb(),
        }
    }
}

impl Default for FileManagerSettings {
    fn default() -> Self {
        Self {
            extended_view_width: default_extended_view_width(),
        }
    }
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            file_path: None,
            min_level: default_min_level(),
            resource_monitor_interval: default_resource_monitor_interval(),
        }
    }
}
