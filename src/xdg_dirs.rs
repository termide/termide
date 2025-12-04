//! XDG Base Directory Specification support
//!
//! This module provides centralized access to XDG directories following
//! the XDG Base Directory Specification:
//! <https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html>
//!
//! Directory structure:
//! - Config: $XDG_CONFIG_HOME/termide (default: ~/.config/termide)
//! - Data: $XDG_DATA_HOME/termide (default: ~/.local/share/termide)
//! - Cache: $XDG_CACHE_HOME/termide (default: ~/.cache/termide)
//! - State: $XDG_STATE_HOME/termide (default: ~/.local/state/termide)

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
/// Used for: session files, unsaved buffers
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

/// Get the cache directory
///
/// Returns `$XDG_CACHE_HOME/termide` or `~/.cache/termide` on Linux/macOS
///
/// Used for: log files, temporary data
///
/// # Examples
///
/// ```
/// let cache_dir = get_cache_dir()?;
/// // Linux: ~/.cache/termide
/// // macOS: ~/Library/Caches/termide
/// // Windows: C:\Users\Username\AppData\Local\termide\cache
/// ```
pub fn get_cache_dir() -> Result<PathBuf> {
    let base_dir = dirs::cache_dir().context("Could not find cache directory")?;
    Ok(base_dir.join("termide"))
}

/// Get the state directory
///
/// Returns `$XDG_STATE_HOME/termide` or `~/.local/state/termide` on Linux
///
/// Used for: application state that should persist but is not critical
///
/// # Examples
///
/// ```
/// let state_dir = get_state_dir()?;
/// // Linux: ~/.local/state/termide
/// // macOS: ~/Library/Application Support/termide (fallback to data_dir)
/// // Windows: C:\Users\Username\AppData\Local\termide\state (fallback)
/// ```
#[allow(dead_code)]
pub fn get_state_dir() -> Result<PathBuf> {
    // dirs::state_dir() is available in dirs 6.0+
    let base_dir = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("Could not find state directory")?;
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
    fn test_get_cache_dir() {
        let dir = get_cache_dir().expect("Failed to get cache dir");
        assert!(dir.to_string_lossy().contains("termide"));
    }

    #[test]
    fn test_get_state_dir() {
        let dir = get_state_dir().expect("Failed to get state dir");
        assert!(dir.to_string_lossy().contains("termide"));
    }

    #[test]
    fn test_directories_are_different() {
        let config = get_config_dir().unwrap();
        let data = get_data_dir().unwrap();
        let cache = get_cache_dir().unwrap();

        // On most systems, these should be different directories
        // (except macOS where some might overlap)
        #[cfg(target_os = "linux")]
        {
            assert_ne!(config, data);
            assert_ne!(config, cache);
            assert_ne!(data, cache);
        }
    }
}
