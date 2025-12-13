//! XDG Base Directory support for termide.

use anyhow::{Context, Result};
use std::path::PathBuf;

const APP_NAME: &str = "termide";

/// Get the configuration directory following XDG conventions.
///
/// Returns `$XDG_CONFIG_HOME/termide` or `~/.config/termide`.
pub fn get_config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|p| p.join(APP_NAME))
        .context("Failed to determine config directory")
}

/// Get the data directory following XDG conventions.
///
/// Returns `$XDG_DATA_HOME/termide` or `~/.local/share/termide`.
pub fn get_data_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|p| p.join(APP_NAME))
        .context("Failed to determine data directory")
}

/// Get the cache directory following XDG conventions.
///
/// Returns `$XDG_CACHE_HOME/termide` or `~/.cache/termide`.
pub fn get_cache_dir() -> Result<PathBuf> {
    dirs::cache_dir()
        .map(|p| p.join(APP_NAME))
        .context("Failed to determine cache directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_dir() {
        let dir = get_config_dir().unwrap();
        assert!(dir.ends_with("termide"));
    }

    #[test]
    fn test_get_data_dir() {
        let dir = get_data_dir().unwrap();
        assert!(dir.ends_with("termide"));
    }

    #[test]
    fn test_directories_are_different() {
        let config = get_config_dir().unwrap();
        let data = get_data_dir().unwrap();
        assert_ne!(config, data);
    }
}
