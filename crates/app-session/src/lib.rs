//! Session management traits and helpers for termide.
//!
//! This crate provides:
//! - `SessionManager` trait for session save/load operations
//! - `SessionState` for tracking session state
//! - `AutoSaveConfig` for configuring automatic saves
//!
//! # Architecture
//!
//! Sessions are saved per-project and track panel layout:
//!
//! ```text
//! Project Root → Session Directory → session.toml
//!                                  → unsaved_buffers/
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;

// ============================================================================
// Session Data Types
// ============================================================================

/// Panel session data (for serialization).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPanel {
    /// Panel type name
    pub panel_type: String,
    /// Panel-specific data
    pub data: SessionPanelData,
}

/// Panel-specific session data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPanelData {
    /// Editor panel data
    Editor {
        /// File path (None for unsaved buffer)
        file_path: Option<PathBuf>,
        /// Unsaved buffer filename (if any)
        buffer_file: Option<String>,
        /// Cursor line
        cursor_line: usize,
        /// Cursor column
        cursor_col: usize,
    },
    /// File manager panel data
    FileManager {
        /// Current directory
        current_dir: PathBuf,
        /// Selected index
        selected_index: usize,
    },
    /// Terminal panel data
    Terminal {
        /// Working directory
        cwd: PathBuf,
    },
    /// Log viewer panel data
    LogViewer,
    /// Welcome panel data
    Welcome,
}

/// Panel group session data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGroup {
    /// Panels in this group
    pub panels: Vec<SessionPanel>,
    /// Expanded panel index
    pub expanded_index: usize,
    /// Group width (None for auto)
    pub width: Option<u16>,
}

/// Complete session data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionData {
    /// Panel groups
    pub groups: Vec<SessionGroup>,
    /// Focused group index
    pub focus: usize,
    /// Session version (for migration)
    pub version: u32,
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            focus: 0,
            version: 1,
        }
    }
}

// ============================================================================
// Session State Tracking
// ============================================================================

/// Tracks session state for debouncing saves.
#[derive(Debug)]
pub struct SessionState {
    /// Last save time
    last_save: Option<Instant>,
    /// Whether session has unsaved changes
    is_dirty: bool,
    /// Debounce duration
    debounce: Duration,
}

impl SessionState {
    /// Create new session state with default debounce.
    pub fn new() -> Self {
        Self {
            last_save: None,
            is_dirty: false,
            debounce: Duration::from_secs(1),
        }
    }

    /// Create with custom debounce duration.
    pub fn with_debounce(debounce: Duration) -> Self {
        Self {
            last_save: None,
            is_dirty: false,
            debounce,
        }
    }

    /// Mark session as dirty (needs save).
    pub fn mark_dirty(&mut self) {
        self.is_dirty = true;
    }

    /// Mark session as saved.
    pub fn mark_saved(&mut self) {
        self.last_save = Some(Instant::now());
        self.is_dirty = false;
    }

    /// Check if session should be saved (dirty + debounce elapsed).
    pub fn should_save(&self) -> bool {
        if !self.is_dirty {
            return false;
        }

        match self.last_save {
            None => true,
            Some(last) => last.elapsed() >= self.debounce,
        }
    }

    /// Check if session is dirty.
    pub fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// Get time since last save.
    pub fn time_since_save(&self) -> Option<Duration> {
        self.last_save.map(|t| t.elapsed())
    }

    /// Reset state.
    pub fn reset(&mut self) {
        self.last_save = None;
        self.is_dirty = false;
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Auto-Save Configuration
// ============================================================================

/// Configuration for automatic session saves.
#[derive(Debug, Clone)]
pub struct AutoSaveConfig {
    /// Enable auto-save on focus loss
    pub on_focus_loss: bool,
    /// Enable auto-save on navigation
    pub on_navigation: bool,
    /// Enable auto-save on panel close
    pub on_panel_close: bool,
    /// Minimum interval between saves
    pub min_interval: Duration,
}

impl Default for AutoSaveConfig {
    fn default() -> Self {
        Self {
            on_focus_loss: true,
            on_navigation: true,
            on_panel_close: true,
            min_interval: Duration::from_secs(1),
        }
    }
}

impl AutoSaveConfig {
    /// Create config with all auto-saves disabled.
    pub fn disabled() -> Self {
        Self {
            on_focus_loss: false,
            on_navigation: false,
            on_panel_close: false,
            min_interval: Duration::from_secs(1),
        }
    }

    /// Create config with minimal saves (focus loss only).
    pub fn minimal() -> Self {
        Self {
            on_focus_loss: true,
            on_navigation: false,
            on_panel_close: false,
            min_interval: Duration::from_secs(5),
        }
    }
}

// ============================================================================
// Session Manager Trait
// ============================================================================

/// Trait for session management operations.
pub trait SessionManager {
    /// Save current session.
    fn save(&mut self) -> Result<()>;

    /// Load session from storage.
    fn load(&self) -> Result<SessionData>;

    /// Check if session should be saved (based on state).
    fn should_save(&self) -> bool;

    /// Mark session as needing save.
    fn mark_dirty(&mut self);

    /// Mark session as saved.
    fn mark_saved(&mut self);

    /// Get session directory for project.
    fn session_dir(&self) -> &Path;
}

/// Trait for converting layout to session data.
pub trait LayoutToSession {
    /// Convert layout to session data.
    fn to_session(&self, session_dir: &Path) -> SessionData;
}

/// Trait for restoring layout from session data.
pub trait SessionToLayout {
    /// Restore layout from session data.
    fn restore_from_session(&mut self, session: SessionData, session_dir: &Path) -> Result<()>;
}

// ============================================================================
// Session Path Helpers
// ============================================================================

/// Get session directory for a project root.
pub fn get_session_dir(project_root: &Path) -> PathBuf {
    // Use project-specific directory in user's config
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("termide")
        .join("sessions");

    // Hash project path for unique directory name
    let hash = simple_hash(project_root.to_string_lossy().as_bytes());
    config_dir.join(format!("{:016x}", hash))
}

/// Simple hash function for path hashing.
fn simple_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    hash
}

/// Get session file path.
pub fn get_session_file(session_dir: &Path) -> PathBuf {
    session_dir.join("session.toml")
}

/// Get unsaved buffers directory.
pub fn get_buffers_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("unsaved_buffers")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_data_default() {
        let data = SessionData::default();
        assert!(data.groups.is_empty());
        assert_eq!(data.focus, 0);
        assert_eq!(data.version, 1);
    }

    #[test]
    fn test_session_panel_editor() {
        let panel = SessionPanel {
            panel_type: "Editor".to_string(),
            data: SessionPanelData::Editor {
                file_path: Some(PathBuf::from("/test.rs")),
                buffer_file: None,
                cursor_line: 10,
                cursor_col: 5,
            },
        };

        assert_eq!(panel.panel_type, "Editor");
        if let SessionPanelData::Editor {
            cursor_line,
            cursor_col,
            ..
        } = panel.data
        {
            assert_eq!(cursor_line, 10);
            assert_eq!(cursor_col, 5);
        } else {
            panic!("Expected Editor data");
        }
    }

    #[test]
    fn test_session_state_new() {
        let state = SessionState::new();
        assert!(!state.is_dirty());
        assert!(state.time_since_save().is_none());
    }

    #[test]
    fn test_session_state_dirty() {
        let mut state = SessionState::new();
        assert!(!state.is_dirty());

        state.mark_dirty();
        assert!(state.is_dirty());
        assert!(state.should_save()); // No previous save, should save immediately
    }

    #[test]
    fn test_session_state_saved() {
        let mut state = SessionState::new();
        state.mark_dirty();
        state.mark_saved();

        assert!(!state.is_dirty());
        assert!(state.time_since_save().is_some());
    }

    #[test]
    fn test_session_state_debounce() {
        let mut state = SessionState::with_debounce(Duration::from_secs(10));

        state.mark_dirty();
        state.mark_saved();

        // Mark dirty again immediately
        state.mark_dirty();

        // Should not save yet (within debounce window)
        assert!(!state.should_save());
    }

    #[test]
    fn test_auto_save_config_default() {
        let config = AutoSaveConfig::default();
        assert!(config.on_focus_loss);
        assert!(config.on_navigation);
        assert!(config.on_panel_close);
    }

    #[test]
    fn test_auto_save_config_disabled() {
        let config = AutoSaveConfig::disabled();
        assert!(!config.on_focus_loss);
        assert!(!config.on_navigation);
        assert!(!config.on_panel_close);
    }

    #[test]
    fn test_auto_save_config_minimal() {
        let config = AutoSaveConfig::minimal();
        assert!(config.on_focus_loss);
        assert!(!config.on_navigation);
        assert!(!config.on_panel_close);
    }

    #[test]
    fn test_get_session_dir() {
        let project = PathBuf::from("/home/user/project");
        let session_dir = get_session_dir(&project);

        assert!(session_dir.to_string_lossy().contains("termide"));
        assert!(session_dir.to_string_lossy().contains("sessions"));
    }

    #[test]
    fn test_get_session_file() {
        let session_dir = PathBuf::from("/config/termide/sessions/abc123");
        let file = get_session_file(&session_dir);
        assert_eq!(
            file,
            PathBuf::from("/config/termide/sessions/abc123/session.toml")
        );
    }

    #[test]
    fn test_get_buffers_dir() {
        let session_dir = PathBuf::from("/config/termide/sessions/abc123");
        let buffers = get_buffers_dir(&session_dir);
        assert_eq!(
            buffers,
            PathBuf::from("/config/termide/sessions/abc123/unsaved_buffers")
        );
    }

    #[test]
    fn test_simple_hash_deterministic() {
        let hash1 = simple_hash(b"test");
        let hash2 = simple_hash(b"test");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_simple_hash_different_inputs() {
        let hash1 = simple_hash(b"test1");
        let hash2 = simple_hash(b"test2");
        assert_ne!(hash1, hash2);
    }
}
