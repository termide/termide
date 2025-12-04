use anyhow::{Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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
    /// Debug log panel
    #[serde(rename = "debug")]
    Debug,
    // Note: Welcome panels are NOT saved (they auto-close)
}

impl Session {
    /// Get the session directory for a specific project
    ///
    /// Creates nested subdirectories matching the project path.
    /// Example: /home/user/project1 -> ~/.local/share/termide/sessions/home/user/project1/
    pub fn get_session_dir(project_root: &Path) -> Result<PathBuf> {
        let data_dir = crate::xdg_dirs::get_data_dir()?;

        // Canonicalize the project path to handle symlinks and relative paths
        let canonical_project = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());

        // Strip the leading "/" or drive letter to create a relative path
        let relative_path = canonical_project
            .strip_prefix("/")
            .unwrap_or(&canonical_project);

        Ok(data_dir.join("sessions").join(relative_path))
    }

    /// Get the path to the session.toml file for a specific project
    pub fn get_session_path(project_root: &Path) -> Result<PathBuf> {
        Ok(Self::get_session_dir(project_root)?.join("session.toml"))
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

/// Generate a unique filename for an unsaved buffer
///
/// Format: unsaved-YYYYMMDD-HHIISS-MSEC.txt
/// Example: unsaved-20251203-143022-456.txt
pub fn generate_unsaved_filename() -> String {
    let now = Local::now();
    let millis = now.timestamp_subsec_millis();
    format!("unsaved-{}-{:03}.txt", now.format("%Y%m%d-%H%M%S"), millis)
}

/// Save unsaved buffer content to a temporary file
pub fn save_unsaved_buffer(session_dir: &Path, filename: &str, content: &str) -> Result<()> {
    let buffer_path = session_dir.join(filename);
    fs::write(&buffer_path, content).with_context(|| {
        format!(
            "Failed to write unsaved buffer file: {}",
            buffer_path.display()
        )
    })?;
    Ok(())
}

/// Load unsaved buffer content from a temporary file
pub fn load_unsaved_buffer(session_dir: &Path, filename: &str) -> Result<String> {
    let buffer_path = session_dir.join(filename);
    fs::read_to_string(&buffer_path).with_context(|| {
        format!(
            "Failed to read unsaved buffer file: {}",
            buffer_path.display()
        )
    })
}

/// Clean up (delete) an unsaved buffer temporary file
pub fn cleanup_unsaved_buffer(session_dir: &Path, filename: &str) -> Result<()> {
    let buffer_path = session_dir.join(filename);
    if buffer_path.exists() {
        fs::remove_file(&buffer_path).with_context(|| {
            format!(
                "Failed to delete unsaved buffer file: {}",
                buffer_path.display()
            )
        })?;
    }
    Ok(())
}

/// Clean up old sessions (excluding the current project's session)
///
/// Removes sessions older than `retention_days` from the sessions directory
pub fn cleanup_old_sessions(current_project: &Path, retention_days: u32) -> Result<()> {
    use std::time::{Duration, SystemTime};

    let data_dir = crate::xdg_dirs::get_data_dir()?;
    let sessions_dir = data_dir.join("sessions");

    if !sessions_dir.exists() {
        return Ok(()); // No sessions to clean up
    }

    // Canonicalize current project path for comparison
    let current_canonical = current_project
        .canonicalize()
        .unwrap_or_else(|_| current_project.to_path_buf());

    let retention_duration = Duration::from_secs(retention_days as u64 * 24 * 60 * 60);
    let cutoff_time = SystemTime::now()
        .checked_sub(retention_duration)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    // Walk through sessions directory recursively
    walk_and_cleanup(&sessions_dir, &current_canonical, cutoff_time)?;

    Ok(())
}

/// Recursively walk through directories and clean up old sessions
fn walk_and_cleanup(
    dir: &Path,
    current_project: &Path,
    cutoff_time: std::time::SystemTime,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // Skip entries we can't read
        };

        let path = entry.path();

        if path.is_dir() {
            // Check if this directory contains session.toml
            let session_file = path.join("session.toml");

            if session_file.exists() {
                // Check if this is the current project's session
                if !is_same_session(&path, current_project) {
                    // Check file modification time
                    if let Ok(metadata) = session_file.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            if modified < cutoff_time {
                                // Remove entire session directory
                                if let Err(e) = fs::remove_dir_all(&path) {
                                    eprintln!(
                                        "Warning: Failed to remove old session {}: {}",
                                        path.display(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            } else {
                // Recurse into subdirectories
                let _ = walk_and_cleanup(&path, current_project, cutoff_time);
            }
        }
    }

    Ok(())
}

/// Check if session directory corresponds to the given project path
fn is_same_session(session_dir: &Path, project_path: &Path) -> bool {
    let data_dir = match crate::xdg_dirs::get_data_dir() {
        Ok(dir) => dir,
        Err(_) => return false,
    };

    let sessions_base = data_dir.join("sessions");

    // Extract relative path from session directory
    let rel_path = match session_dir.strip_prefix(&sessions_base) {
        Ok(p) => p,
        Err(_) => return false,
    };

    // Reconstruct full path
    let reconstructed = PathBuf::from("/").join(rel_path);

    // Canonicalize both paths for comparison
    let reconstructed_canonical = reconstructed.canonicalize().unwrap_or(reconstructed);
    let project_canonical = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());

    reconstructed_canonical == project_canonical
}

/// Clean up orphaned unsaved buffer files (not referenced in session.toml)
///
/// This removes temporary files that are no longer needed because:
/// - The editor was closed
/// - The buffer was saved to a real file
/// - The session was corrupted or manually edited
pub fn cleanup_orphaned_buffers(session_dir: &Path) -> Result<()> {
    use std::collections::HashSet;

    if !session_dir.exists() {
        return Ok(()); // Nothing to clean
    }

    // Load session to get list of active buffer files
    let session_file = session_dir.join("session.toml");
    let active_buffers: HashSet<String> = if session_file.exists() {
        match fs::read_to_string(&session_file) {
            Ok(contents) => match toml::from_str::<Session>(&contents) {
                Ok(session) => {
                    // Collect all unsaved_buffer_file references from session
                    session
                        .panel_groups
                        .iter()
                        .flat_map(|group| &group.panels)
                        .filter_map(|panel| match panel {
                            SessionPanel::Editor {
                                unsaved_buffer_file,
                                ..
                            } => unsaved_buffer_file.clone(),
                            _ => None,
                        })
                        .collect()
                }
                Err(_) => HashSet::new(), // Failed to parse, proceed with cleanup
            },
            Err(_) => HashSet::new(), // Failed to read, proceed with cleanup
        }
    } else {
        HashSet::new() // No session file, clean all temporary files
    };

    // Find all unsaved-*.txt files in session directory
    let entries = match fs::read_dir(session_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()), // Can't read directory, skip cleanup
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            // Check if this is an unsaved buffer file
            if filename.starts_with("unsaved-") && filename.ends_with(".txt") {
                // If not in active list, delete it
                if !active_buffers.contains(filename) {
                    if let Err(e) = fs::remove_file(&path) {
                        eprintln!(
                            "Warning: Failed to remove orphaned buffer file {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }
    }

    Ok(())
}
