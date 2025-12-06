//! XDG Base Directory Specification support
//!
//! This module provides centralized access to XDG directories following
//! the XDG Base Directory Specification:
//! <https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html>
//!
//! Directory structure:
//! - Config: $XDG_CONFIG_HOME/termide (default: ~/.config/termide)
//! - Data: $XDG_DATA_HOME/termide (default: ~/.local/share/termide)

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get the configuration directory
///
/// Returns `$XDG_CONFIG_HOME/termide` or `~/.config/termide` on Linux/macOS
///
/// # Examples
///
/// ```
/// let config_dir = get_config_dir()?;
/// // Linux: ~/.config/termide
/// // macOS: ~/Library/Application Support/termide
/// // Windows: C:\Users\Username\AppData\Roaming\termide
/// ```
pub fn get_config_dir() -> Result<PathBuf> {
    let base_dir = dirs::config_dir().context("Could not find config directory")?;
    Ok(base_dir.join("termide"))
}

/// Get the data directory
///
/// Returns `$XDG_DATA_HOME/termide` or `~/.local/share/termide` on Linux/macOS
///
/// Used for: session files, unsaved buffers, logs
///
/// # Examples
///
/// ```
/// let data_dir = get_data_dir()?;
/// // Linux: ~/.local/share/termide
/// // macOS: ~/Library/Application Support/termide
/// // Windows: C:\Users\Username\AppData\Roaming\termide
/// ```
pub fn get_data_dir() -> Result<PathBuf> {
    let base_dir = dirs::data_dir().context("Could not find data directory")?;
    Ok(base_dir.join("termide"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_dir() {
        let dir = get_config_dir().expect("Failed to get config dir");
        assert!(dir.to_string_lossy().contains("termide"));
    }

    #[test]
    fn test_get_data_dir() {
        let dir = get_data_dir().expect("Failed to get data dir");
        assert!(dir.to_string_lossy().contains("termide"));
    }

    #[test]
    fn test_directories_are_different() {
        let config = get_config_dir().unwrap();
        let data = get_data_dir().unwrap();

        // On most systems, these should be different directories
        #[cfg(target_os = "linux")]
        {
            assert_ne!(config, data);
        }
    }
}
