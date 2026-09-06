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

    /// XDG keeps configuration and data apart (`~/.config` vs
    /// `~/.local/share`), and code that writes to one must not land in the
    /// other.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_directories_are_different() {
        let config = get_config_dir().unwrap();
        let data = get_data_dir().unwrap();
        assert_ne!(config, data);
    }

    /// macOS deliberately has no such split: Apple's convention puts both
    /// under `~/Library/Application Support`, which is what `dirs` returns
    /// and what CONTRIBUTING.md documents. The two are safe to share because
    /// their contents do not collide — `config.toml` and `themes/` on one
    /// side, `sessions/` on the other. Pinned here so the coincidence stays a
    /// decision rather than a surprise.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_directories_coincide_on_macos() {
        let config = get_config_dir().unwrap();
        let data = get_data_dir().unwrap();
        assert_eq!(config, data);
        assert!(config.ends_with("termide"));
    }
}
