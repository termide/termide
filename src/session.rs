use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Session state for saving and restoring panel layout
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Panel groups (vertical columns with accordion)
    pub panel_groups: Vec<SessionPanelGroup>,
    /// Which group is currently focused (0-based index)
    pub focused_group: usize,
    /// FileManager current path (if exists)
    pub file_manager_path: Option<PathBuf>,
}

/// A group of panels (one vertical column)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPanelGroup {
    /// Panels in this group
    pub panels: Vec<SessionPanel>,
    /// Which panel is expanded (0-based index)
    pub expanded_index: usize,
    /// Column width in characters (None = auto-distributed)
    pub width: Option<u16>,
}

/// Panel data for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionPanel {
    /// File manager panel
    #[serde(rename = "file_manager")]
    FileManager {
        /// Current directory path
        path: PathBuf,
    },
    /// Text editor panel
    #[serde(rename = "editor")]
    Editor {
        /// File path (None for unnamed/scratch buffers)
        path: Option<PathBuf>,
    },
    /// Terminal panel
    #[serde(rename = "terminal")]
    Terminal {
        /// Working directory
        working_dir: PathBuf,
    },
    /// Debug log panel
    #[serde(rename = "debug")]
    Debug,
    // Note: Welcome panels are NOT saved (they auto-close)
}

impl Session {
    /// Get the path to the session file
    pub fn get_session_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Could not find config directory")?;
        Ok(config_dir.join("termide").join("session.toml"))
    }

    /// Load session from file
    pub fn load() -> Result<Self> {
        let path = Self::get_session_path()?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;
        let session: Session = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse session file: {}", path.display()))?;
        Ok(session)
    }

    /// Save session to file
    pub fn save(&self) -> Result<()> {
        let path = Self::get_session_path()?;

        // Ensure config directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        let contents = toml::to_string_pretty(self).context("Failed to serialize session")?;

        fs::write(&path, contents)
            .with_context(|| format!("Failed to write session file: {}", path.display()))?;

        Ok(())
    }
}
