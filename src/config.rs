use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Application configuration with nested sections
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

/// General application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    /// Selected theme name
    #[serde(default = "default_theme_name")]
    pub theme: String,

    /// Interface language (en, de, es, fr, hi, pt, ru, th, zh, or auto for auto-detection)
    #[serde(default = "default_language")]
    pub language: String,

    /// Minimum panel width in characters (default: 80)
    /// Panels narrower than this threshold will be stacked vertically
    #[serde(default = "default_min_panel_width")]
    pub min_panel_width: u16,

    /// Session retention period in days (default: 30)
    /// Sessions older than this will be automatically deleted on startup
    #[serde(default = "default_session_retention_days")]
    pub session_retention_days: u32,
}

/// Editor settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    /// Tab size (number of spaces)
    #[serde(default = "default_tab_size")]
    pub tab_size: usize,

    /// Show git diff status colors on line numbers in editor (default: true)
    #[serde(default = "default_show_git_diff")]
    pub show_git_diff: bool,

    /// Enable word wrap in editor (default: true)
    /// When enabled, long lines are automatically wrapped to fit viewport width
    #[serde(default = "default_word_wrap")]
    pub word_wrap: bool,

    /// File size threshold in MB for enabling smart features (default: 5)
    /// Files larger than this threshold will disable expensive features like
    /// smart word wrapping to maintain performance
    #[serde(default = "default_large_file_threshold_mb")]
    pub large_file_threshold_mb: u64,
}

/// File manager settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManagerSettings {
    /// Minimum file manager panel width to display size and time columns (default: 50)
    #[serde(default = "default_extended_view_width")]
    pub extended_view_width: usize,
}

/// Logging settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingSettings {
    /// Log file path (if not specified, temporary directory is used)
    #[serde(default)]
    pub file_path: Option<String>,

    /// Minimum log level (default: "info")
    /// Possible values: "debug", "info", "warn", "error"
    /// Logs below this level will not be recorded
    #[serde(default = "default_min_level")]
    pub min_level: String,

    /// System resource monitor update interval in milliseconds (default: 1000ms)
    #[serde(default = "default_resource_monitor_interval")]
    pub resource_monitor_interval: u64,
}

// Default value functions
fn default_theme_name() -> String {
    "default".to_string()
}

fn default_language() -> String {
    "auto".to_string()
}

fn default_min_panel_width() -> u16 {
    80
}

fn default_session_retention_days() -> u32 {
    30
}

fn default_tab_size() -> usize {
    4
}

fn default_show_git_diff() -> bool {
    true
}

fn default_word_wrap() -> bool {
    true
}

fn default_large_file_threshold_mb() -> u64 {
    crate::constants::DEFAULT_LARGE_FILE_THRESHOLD_MB
}

fn default_extended_view_width() -> usize {
    50
}

fn default_min_level() -> String {
    "info".to_string()
}

fn default_resource_monitor_interval() -> u64 {
    1000
}

// Default implementations for nested structs
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

/// Legacy flat config format for migration
#[derive(Debug, Clone, Deserialize)]
struct LegacyConfig {
    #[serde(default = "default_theme_name")]
    theme: String,
    #[serde(default = "default_tab_size")]
    tab_size: usize,
    #[serde(default = "default_language")]
    language: String,
    #[serde(default)]
    log_file_path: Option<String>,
    #[serde(default = "default_resource_monitor_interval")]
    resource_monitor_interval: u64,
    #[serde(default = "default_min_panel_width")]
    min_panel_width: u16,
    #[serde(default = "default_show_git_diff")]
    show_git_diff: bool,
    #[serde(default = "default_extended_view_width")]
    fm_extended_view_width: usize,
    #[serde(default = "default_session_retention_days")]
    session_retention_days: u32,
    #[serde(default = "default_word_wrap")]
    word_wrap: bool,
    #[serde(default = "default_min_level")]
    min_log_level: String,
    #[serde(default = "default_large_file_threshold_mb")]
    large_file_threshold_mb: u64,
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

impl Config {
    /// Load configuration from file
    /// On first run, creates config file with default values
    /// Auto-completes missing keys with default values
    /// Supports migration from legacy flat format
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if config_path.exists() {
            let original_content = std::fs::read_to_string(&config_path)?;

            // Try parsing as new structured format first
            let config: Self = match toml::from_str(&original_content) {
                Ok(config) => config,
                Err(_) => {
                    // Try parsing as legacy flat format
                    let legacy: LegacyConfig = toml::from_str(&original_content)?;
                    let config: Config = legacy.into();
                    // Save in new format
                    config.save()?;
                    return Ok(config);
                }
            };

            // Serialize the config back to TOML to get normalized content
            let normalized_content = toml::to_string_pretty(&config)?;

            // Compute hash of original content
            let mut original_hasher = DefaultHasher::new();
            original_content.hash(&mut original_hasher);
            let original_hash = original_hasher.finish();

            // Compute hash of normalized content
            let mut normalized_hasher = DefaultHasher::new();
            normalized_content.hash(&mut normalized_hasher);
            let normalized_hash = normalized_hasher.finish();

            // If hashes differ, the config was auto-completed with default values
            // Save the updated config to persist the changes
            if original_hash != normalized_hash {
                config.save()?;
            }

            Ok(config)
        } else {
            // First run - create config file with default values
            let config = Self::default();
            config.save()?;

            // Create themes directory
            Self::ensure_themes_dir()?;

            Ok(config)
        }
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        let config_path = Self::get_config_path()?;

        // Create directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(config_path, content)?;

        Ok(())
    }

    /// Get path to config file
    fn get_config_path() -> Result<PathBuf> {
        let config_dir = crate::xdg_dirs::get_config_dir()?;
        Ok(config_dir.join("config.toml"))
    }

    /// Get path to config directory (for debugging)
    #[allow(dead_code)]
    pub fn get_config_dir() -> Result<PathBuf> {
        crate::xdg_dirs::get_config_dir()
    }

    /// Get path to themes directory
    pub fn get_themes_dir() -> Result<PathBuf> {
        Ok(Self::get_config_dir()?.join("themes"))
    }

    /// Ensure themes directory exists
    fn ensure_themes_dir() -> Result<()> {
        let themes_dir = Self::get_themes_dir()?;
        if !themes_dir.exists() {
            std::fs::create_dir_all(themes_dir)?;
        }
        Ok(())
    }

    /// Get path to config file (public version)
    pub fn config_file_path() -> Result<PathBuf> {
        Self::get_config_path()
    }

    /// Check if path is a config file
    pub fn is_config_file(path: &std::path::Path) -> bool {
        if let Ok(config_path) = Self::get_config_path() {
            path == config_path
        } else {
            false
        }
    }

    /// Validate config content
    /// Returns Ok(Config) if validation succeeds, otherwise error
    pub fn validate_content(content: &str) -> Result<Config> {
        toml::from_str(content).map_err(|e| anyhow::anyhow!("{}", e))
    }
}
