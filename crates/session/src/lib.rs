//! Session persistence for termide.
//!
//! Saves and restores application state between runs.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

mod maintenance;
pub use maintenance::*;

/// Session state for saving and restoring panel layout
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Panel groups (vertical columns with accordion)
    pub panel_groups: Vec<SessionPanelGroup>,
    /// Which group is currently focused (0-based index)
    pub focused_group: usize,
}

/// Legacy layout-mode tag retained for backward compatibility with
/// sessions saved before the unified-split refactor. Newer code never
/// writes this field; older sessions deserialize the tag and the
/// loader treats `Accordion` as a request to apply the
/// fullscreen-current-panel preset on top of `expanded_index`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionGroupMode {
    #[default]
    #[serde(rename = "accordion")]
    Accordion,
    #[serde(rename = "split")]
    Split,
}

/// A group of panels (one vertical column).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPanelGroup {
    /// Panels in this group.
    pub panels: Vec<SessionPanel>,
    /// Which panel is focused (0-based index).
    pub expanded_index: usize,
    /// Column width in characters (None = auto-distributed).
    pub width: Option<u16>,
    /// Legacy mode tag — still parsed from old sessions to drive
    /// fullscreen-preset migration. New sessions never write it.
    #[serde(default, skip_serializing)]
    pub mode: SessionGroupMode,
    /// Cached panel heights (in lines). `None` means "no cache yet —
    /// derive equal distribution on first use".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_heights: Option<Vec<u16>>,
    /// When `Some`, the group is in the fullscreen-current-panel preset
    /// and this is the heights snapshot to restore on toggle-off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullscreen_cache: Option<Vec<u16>>,
}

/// Panel data for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionPanel {
    /// File manager panel
    #[serde(rename = "file_manager")]
    FileManager {
        /// Path for local filesystem, or VFS URL for remote (e.g., "sftp://user@host/path")
        #[serde(alias = "path")] // Support old format for backward compatibility
        path_or_url: String,
    },
    /// Text editor panel
    #[serde(rename = "editor")]
    Editor {
        /// File path (None for unnamed/scratch buffers)
        path: Option<PathBuf>,
        /// Temporary file name for unsaved buffers (format: unsaved-YYYYMMDD-HHIISS-MSEC.txt)
        #[serde(skip_serializing_if = "Option::is_none")]
        unsaved_buffer_file: Option<String>,
    },
    /// Terminal panel
    #[serde(rename = "terminal")]
    Terminal {
        /// Working directory
        working_dir: PathBuf,
    },
    /// Journal panel
    #[serde(rename = "journal")]
    Journal,
    /// Image viewer panel
    #[serde(rename = "image")]
    Image {
        /// Path to image file
        path: PathBuf,
    },
    /// Binary hex/ASCII viewer panel
    #[serde(rename = "binary")]
    Binary {
        /// Path to the binary file
        path: PathBuf,
    },
    /// Rendered Markdown preview panel
    #[serde(rename = "markdown")]
    Markdown {
        /// Path to the markdown file
        path: PathBuf,
    },
    /// Mermaid diagram viewer panel
    #[serde(rename = "mermaid")]
    Mermaid {
        /// Path to the `.mmd` file
        path: PathBuf,
    },
    /// Rendered HTML viewer panel
    #[serde(rename = "html")]
    Html {
        /// Path to the HTML file
        path: PathBuf,
    },
    /// Git status panel
    #[serde(rename = "git_status")]
    GitStatus {
        /// Repository path
        repo_path: PathBuf,
    },
    /// Git log panel
    #[serde(rename = "git_log")]
    GitLog {
        /// Repository path
        repo_path: PathBuf,
    },
    /// Git diff panel
    #[serde(rename = "git_diff")]
    GitDiff {
        /// Repository path
        repo_path: PathBuf,
        /// Commit hash (None = working directory changes, Some = specific commit)
        #[serde(skip_serializing_if = "Option::is_none")]
        commit_hash: Option<String>,
    },
    /// Outline panel (symbol navigator)
    #[serde(rename = "outline")]
    Outline,
    /// Diagnostics panel
    #[serde(rename = "diagnostics")]
    Diagnostics,
    /// Database viewer panel
    #[serde(rename = "database")]
    Database {
        /// Connection URL (as entered in the bookmark)
        url: String,
        /// Display label
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label: String,
    },
    // Note: Welcome panels are NOT saved (they auto-close)
}

/// Get the data directory for termide.
pub(crate) fn get_data_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|p| p.join("termide"))
        .context("Failed to determine data directory")
}

impl Session {
    /// Get the session directory for a specific project
    ///
    /// Creates nested subdirectories matching the project path with root stripped.
    /// Example (Unix):    /home/user/project1 -> ~/.local/share/termide/sessions/home/user/project1/
    /// Example (Windows): C:\Users\user\proj  -> %APPDATA%\termide\sessions\Users\user\proj\
    pub fn get_session_dir(project_root: &Path) -> Result<PathBuf> {
        let data_dir = get_data_dir()?;

        // Canonicalize the project path to handle symlinks and relative paths
        let canonical_project = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());

        // Strip root components (leading "/" on Unix; drive prefix + "\" on Windows)
        // so that PathBuf::join does not treat the result as absolute and replace the base.
        // Component::Prefix covers "C:" / "\\server\share"; Component::RootDir covers "/" or "\".
        let relative_path: PathBuf = canonical_project
            .components()
            .filter(|c| {
                !matches!(
                    c,
                    std::path::Component::Prefix(_) | std::path::Component::RootDir
                )
            })
            .collect();

        Ok(data_dir.join("sessions").join(relative_path))
    }

    /// Get the path to the session.toml file for a specific project
    pub fn get_session_path(project_root: &Path) -> Result<PathBuf> {
        Ok(Self::get_session_dir(project_root)?.join("session.toml"))
    }

    /// Delete session directory for a specific project
    pub fn delete_session(project_root: &Path) -> Result<()> {
        let session_dir = Self::get_session_dir(project_root)?;
        if session_dir.exists() {
            fs::remove_dir_all(&session_dir)
                .with_context(|| format!("Failed to delete session: {}", session_dir.display()))?;
        }
        Ok(())
    }

    /// Load session from file for a specific project
    pub fn load(project_root: &Path) -> Result<Self> {
        let path = Self::get_session_path(project_root)?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;
        let session: Session = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse session file: {}", path.display()))?;
        Ok(session)
    }

    /// Save session to file for a specific project
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let session_dir = Self::get_session_dir(project_root)?;

        // Ensure session directory exists
        fs::create_dir_all(&session_dir).with_context(|| {
            format!(
                "Failed to create session directory: {}",
                session_dir.display()
            )
        })?;

        let path = session_dir.join("session.toml");
        let contents = toml::to_string_pretty(self).context("Failed to serialize session")?;

        fs::write(&path, contents)
            .with_context(|| format!("Failed to write session file: {}", path.display()))?;

        Ok(())
    }
}
