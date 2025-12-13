//! Configuration management for termide.
//!
//! This crate provides configuration loading, saving, and validation
//! with support for TOML format and XDG directory conventions.

pub mod constants;
mod settings;
mod xdg;

pub use settings::{
    Config, EditorSettings, FileManagerSettings, GeneralSettings, LegacyConfig, LoggingSettings,
};
pub use xdg::{get_cache_dir, get_config_dir, get_data_dir};

use anyhow::Result;
use std::path::PathBuf;

/// Default values as constants
pub mod defaults {
    pub const THEME_NAME: &str = "default";
    pub const LANGUAGE: &str = "auto";
    pub const MIN_PANEL_WIDTH: u16 = 80;
    pub const SESSION_RETENTION_DAYS: u32 = 30;
    pub const TAB_SIZE: usize = 4;
    pub const SHOW_GIT_DIFF: bool = true;
    pub const WORD_WRAP: bool = true;
    pub const LARGE_FILE_THRESHOLD_MB: u64 = 5;
    pub const EXTENDED_VIEW_WIDTH: usize = 50;
    pub const MIN_LOG_LEVEL: &str = "info";
    pub const RESOURCE_MONITOR_INTERVAL: u64 = 1000;
}

impl Config {
    /// Load configuration from file.
    ///
    /// On first run, creates config file with default values.
    /// Auto-completes missing keys with default values.
    /// Supports migration from legacy flat format.
    pub fn load() -> Result<Self> {
        let config_path = Self::config_file_path()?;

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

            // Serialize back to get normalized content
            let normalized_content = toml::to_string_pretty(&config)?;

            // If content changed, save the updated config
            if original_content != normalized_content {
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

    /// Save configuration to file.
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_file_path()?;

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(config_path, content)?;
        Ok(())
    }

    /// Get path to config file.
    pub fn config_file_path() -> Result<PathBuf> {
        Ok(get_config_dir()?.join("config.toml"))
    }

    /// Get path to themes directory.
    pub fn get_themes_dir() -> Result<PathBuf> {
        Ok(get_config_dir()?.join("themes"))
    }

    /// Check if path is the config file.
    pub fn is_config_file(path: &std::path::Path) -> bool {
        Self::config_file_path().map(|p| p == path).unwrap_or(false)
    }

    /// Validate config content.
    pub fn validate_content(content: &str) -> Result<Config> {
        toml::from_str(content).map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Ensure themes directory exists.
    fn ensure_themes_dir() -> Result<()> {
        let themes_dir = Self::get_themes_dir()?;
        if !themes_dir.exists() {
            std::fs::create_dir_all(themes_dir)?;
        }
        Ok(())
    }
}
