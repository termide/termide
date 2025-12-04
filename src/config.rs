use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Selected theme name
    #[serde(default = "default_theme_name")]
    pub theme: String,

    /// Tab size (number of spaces)
    #[serde(default = "default_tab_size")]
    pub tab_size: usize,

    /// Interface language (en, de, es, fr, hi, pt, ru, th, zh, or auto for auto-detection)
    #[serde(default = "default_language")]
    pub language: String,

    /// Log file path (if not specified, temporary directory is used)
    #[serde(default)]
    pub log_file_path: Option<String>,

    /// System resource monitor update interval in milliseconds (default: 1000ms)
    #[serde(default = "default_resource_monitor_interval")]
    pub resource_monitor_interval: u64,

    /// Minimum panel width in characters (default: 80)
    /// Panels narrower than this threshold will be stacked vertically
    #[serde(default = "default_min_panel_width")]
    pub min_panel_width: u16,

    /// Show git diff status colors on line numbers in editor (default: true)
    #[serde(default = "default_show_git_diff")]
    pub show_git_diff: bool,

    /// Minimum file manager panel width to display size and time columns (default: 50)
    #[serde(default = "default_fm_extended_view_width")]
    pub fm_extended_view_width: usize,

    /// Session retention period in days (default: 30)
    /// Sessions older than this will be automatically deleted on startup
    #[serde(default = "default_session_retention_days")]
    pub session_retention_days: u32,

    /// Enable word wrap in editor (default: true)
    /// When enabled, long lines are automatically wrapped to fit viewport width
    #[serde(default = "default_word_wrap")]
    pub word_wrap: bool,

    /// Minimum log level (default: "info")
    /// Possible values: "debug", "info", "warn", "error"
    /// Logs below this level will not be recorded
    #[serde(default = "default_min_log_level")]
    pub min_log_level: String,
}

fn default_theme_name() -> String {
    "default".to_string()
}

fn default_tab_size() -> usize {
    4
}

fn default_language() -> String {
    "auto".to_string()
}

fn default_resource_monitor_interval() -> u64 {
    1000 // 1 second
}

fn default_min_panel_width() -> u16 {
    80
}

fn default_show_git_diff() -> bool {
    true
}

fn default_fm_extended_view_width() -> usize {
    50
}

fn default_session_retention_days() -> u32 {
    30 // 30 days
}

fn default_word_wrap() -> bool {
    true // Enabled by default
}

fn default_min_log_level() -> String {
    "info".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme_name(),
            tab_size: default_tab_size(),
            language: default_language(),
            log_file_path: None,
            resource_monitor_interval: default_resource_monitor_interval(),
            min_panel_width: default_min_panel_width(),
            show_git_diff: default_show_git_diff(),
            fm_extended_view_width: default_fm_extended_view_width(),
            session_retention_days: default_session_retention_days(),
            word_wrap: default_word_wrap(),
            min_log_level: default_min_log_level(),
        }
    }
}

impl Config {
    /// Load configuration from file
    /// On first run, creates config file with default values
    /// Auto-completes missing keys with default values
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if config_path.exists() {
            // Read existing config file
            let original_content = std::fs::read_to_string(&config_path)?;

            // Deserialize config (missing fields will use defaults from serde)
            let config: Self = toml::from_str(&original_content)?;

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

    /// Get path to log file
    /// If specified in config, use it; otherwise use XDG cache directory
    pub fn get_log_file_path(&self) -> PathBuf {
        if let Some(ref path) = self.log_file_path {
            PathBuf::from(path)
        } else {
            // By default use XDG cache directory (~/.cache/termide on Linux)
            crate::xdg_dirs::get_cache_dir()
                .map(|dir| dir.join("termide.log"))
                .unwrap_or_else(|_| std::env::temp_dir().join("termide.log"))
        }
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
