use anyhow::Result;
use serde::{Deserialize, Serialize};
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

    /// Interface language (en, ru, or auto for auto-detection)
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

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme_name(),
            tab_size: default_tab_size(),
            language: default_language(),
            log_file_path: None,
            resource_monitor_interval: default_resource_monitor_interval(),
            min_panel_width: default_min_panel_width(),
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

            // Check if any config keys are missing from the original file
            let required_keys = [
                "theme",
                "tab_size",
                "language",
                "resource_monitor_interval",
                "min_panel_width",
            ];

            let needs_update = required_keys
                .iter()
                .any(|key| !original_content.contains(key));

            // If any keys are missing, save the complete config
            if needs_update {
                let _ = config.save(); // Ignore save error
            }

            Ok(config)
        } else {
            // First run - create config file with default values
            let config = Self::default();
            let _ = config.save(); // Ignore save error

            // Create themes directory
            let _ = Self::ensure_themes_dir();

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
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;

        Ok(config_dir.join("termide").join("config.toml"))
    }

    /// Get path to config directory (for debugging)
    #[allow(dead_code)]
    pub fn get_config_dir() -> Result<PathBuf> {
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;

        Ok(config_dir.join("termide"))
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
    /// If specified in config, use it; otherwise use temporary directory
    pub fn get_log_file_path(&self) -> PathBuf {
        if let Some(ref path) = self.log_file_path {
            PathBuf::from(path)
        } else {
            // By default use temporary directory
            std::env::temp_dir().join("termide.log")
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
