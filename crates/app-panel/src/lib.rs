//! Panel factory and lifecycle management for termide.
//!
//! This crate provides:
//! - `PanelFactory` trait for creating panel instances
//! - `PanelLifecycle` trait for managing panel close/cleanup
//! - `CloseDecision` enum for close confirmation results
//!
//! # Architecture
//!
//! Panel creation follows a factory pattern that abstracts the concrete
//! panel types, enabling testability and future plugin support.
//!
//! ```text
//! PanelType → PanelFactory → Box<dyn Panel>
//!                  ↓
//!             Configuration
//! ```

use std::path::PathBuf;

use anyhow::Result;

use termide_app_core::{Panel, PanelType};

// ============================================================================
// Panel Creation Configuration
// ============================================================================

/// Configuration for creating an editor panel.
#[derive(Debug, Clone, Default)]
pub struct EditorCreationConfig {
    /// File to open (None for new buffer)
    pub file_path: Option<PathBuf>,
    /// Tab size in spaces
    pub tab_size: usize,
    /// Enable word wrap
    pub word_wrap: bool,
}

/// Configuration for creating a terminal panel.
#[derive(Debug, Clone, Default)]
pub struct TerminalCreationConfig {
    /// Working directory
    pub cwd: Option<PathBuf>,
    /// Terminal height in rows
    pub height: u16,
    /// Terminal width in columns
    pub width: u16,
}

/// Configuration for creating a file manager panel.
#[derive(Debug, Clone, Default)]
pub struct FileManagerCreationConfig {
    /// Starting directory
    pub working_dir: Option<PathBuf>,
    /// Show hidden files
    pub show_hidden: bool,
}

/// Unified panel creation configuration.
#[derive(Debug, Clone)]
pub enum PanelCreationConfig {
    /// Create editor panel
    Editor(EditorCreationConfig),
    /// Create terminal panel
    Terminal(TerminalCreationConfig),
    /// Create file manager panel
    FileManager(FileManagerCreationConfig),
    /// Create log viewer panel (no config needed)
    LogViewer,
    /// Create welcome panel (no config needed)
    Welcome,
}

impl From<PanelType> for PanelCreationConfig {
    fn from(panel_type: PanelType) -> Self {
        match panel_type {
            PanelType::Editor { file_path } => PanelCreationConfig::Editor(EditorCreationConfig {
                file_path,
                ..Default::default()
            }),
            PanelType::Terminal { cwd } => PanelCreationConfig::Terminal(TerminalCreationConfig {
                cwd,
                ..Default::default()
            }),
            PanelType::FileManager { working_dir } => {
                PanelCreationConfig::FileManager(FileManagerCreationConfig {
                    working_dir,
                    ..Default::default()
                })
            }
            PanelType::LogViewer => PanelCreationConfig::LogViewer,
            PanelType::Welcome => PanelCreationConfig::Welcome,
        }
    }
}

// ============================================================================
// Panel Factory Trait
// ============================================================================

/// Trait for creating panel instances.
///
/// Implementations provide concrete panel creation logic,
/// allowing the app orchestrator to remain decoupled from
/// specific panel implementations.
pub trait PanelFactory {
    /// Create a panel from configuration.
    fn create(&self, config: PanelCreationConfig) -> Result<Box<dyn Panel>>;

    /// Create an editor panel.
    fn create_editor(&self, config: EditorCreationConfig) -> Result<Box<dyn Panel>>;

    /// Create a terminal panel.
    fn create_terminal(&self, config: TerminalCreationConfig) -> Result<Box<dyn Panel>>;

    /// Create a file manager panel.
    fn create_file_manager(&self, config: FileManagerCreationConfig) -> Result<Box<dyn Panel>>;

    /// Create a log viewer panel.
    fn create_log_viewer(&self) -> Result<Box<dyn Panel>>;

    /// Create a welcome panel.
    fn create_welcome(&self) -> Result<Box<dyn Panel>>;
}

// ============================================================================
// Panel Lifecycle
// ============================================================================

/// Decision about whether a panel can be closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseDecision {
    /// Panel can be closed immediately
    Close,
    /// Panel needs user confirmation before closing
    NeedsConfirmation {
        /// Reason for requiring confirmation
        reason: String,
        /// Type of confirmation needed
        confirmation_type: ConfirmationType,
    },
    /// Close operation was cancelled
    Cancel,
}

/// Type of confirmation needed for closing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationType {
    /// Simple yes/no confirmation
    Simple,
    /// Editor with unsaved changes (save/discard/cancel)
    UnsavedChanges,
    /// Editor with external changes (overwrite/reload/cancel)
    ExternalChanges,
    /// Editor with both local and external changes
    ConflictingChanges,
}

/// Result of panel cleanup operation.
#[derive(Debug, Clone, Default)]
pub struct CleanupResult {
    /// Files that were deleted during cleanup
    pub deleted_files: Vec<PathBuf>,
    /// Directories that were unwatched
    pub unwatched_paths: Vec<PathBuf>,
    /// Any warnings generated during cleanup
    pub warnings: Vec<String>,
}

/// Trait for managing panel lifecycle events.
///
/// Implementations handle panel close confirmation and cleanup.
pub trait PanelLifecycle {
    /// Check if panel can be closed.
    ///
    /// Returns decision about whether close is allowed,
    /// needs confirmation, or should be cancelled.
    fn prepare_close(&self, panel: &dyn Panel) -> CloseDecision;

    /// Perform cleanup before panel is closed.
    ///
    /// Called after user confirms close (or if no confirmation needed).
    /// Should clean up resources like temporary files, watchers, etc.
    fn cleanup(&mut self, panel: &mut dyn Panel) -> CleanupResult;

    /// Called after panel is closed.
    ///
    /// Use for post-close actions like refreshing other panels.
    fn after_close(&mut self, panel_type: &str);
}

// ============================================================================
// Panel Query Helpers
// ============================================================================

/// Information about a panel for queries.
#[derive(Debug, Clone)]
pub struct PanelInfo {
    /// Panel type name
    pub type_name: String,
    /// Panel title
    pub title: String,
    /// Working directory if applicable
    pub working_dir: Option<PathBuf>,
    /// Whether panel has unsaved changes
    pub has_unsaved_changes: bool,
    /// Whether panel is a singleton (only one instance)
    pub is_singleton: bool,
}

/// Trait for querying panel information.
///
/// Provides a way to get panel metadata without coupling
/// to specific panel implementations.
pub trait PanelQuery {
    /// Get information about a panel.
    fn get_info(&self, panel: &dyn Panel) -> PanelInfo;

    /// Check if panel is of a specific type.
    fn is_type(&self, panel: &dyn Panel, type_name: &str) -> bool;

    /// Get working directory from panel if available.
    fn get_working_directory(&self, panel: &dyn Panel) -> Option<PathBuf>;
}

// ============================================================================
// Navigation Manager
// ============================================================================

/// Direction for panel navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    /// Previous group (horizontal)
    PrevGroup,
    /// Next group (horizontal)
    NextGroup,
    /// Previous panel in group (vertical)
    PrevInGroup,
    /// Next panel in group (vertical)
    NextInGroup,
    /// First group
    First,
    /// Last group
    Last,
}

/// Result of navigation operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationResult {
    /// Navigation succeeded
    Success,
    /// Navigation failed (e.g., already at boundary)
    AtBoundary,
    /// No panels to navigate to
    NoPanels,
}

/// Trait for panel navigation operations.
pub trait PanelNavigator {
    /// Navigate in specified direction.
    fn navigate(&mut self, direction: NavigationDirection) -> NavigationResult;

    /// Navigate to specific panel by index.
    fn navigate_to(&mut self, index: usize) -> NavigationResult;

    /// Get current panel index.
    fn current_index(&self) -> Option<usize>;

    /// Get total panel count.
    fn panel_count(&self) -> usize;
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_close_decision_variants() {
        let close = CloseDecision::Close;
        assert_eq!(close, CloseDecision::Close);

        let needs_confirm = CloseDecision::NeedsConfirmation {
            reason: "Unsaved changes".to_string(),
            confirmation_type: ConfirmationType::UnsavedChanges,
        };
        assert!(matches!(
            needs_confirm,
            CloseDecision::NeedsConfirmation { .. }
        ));

        let cancel = CloseDecision::Cancel;
        assert_eq!(cancel, CloseDecision::Cancel);
    }

    #[test]
    fn test_confirmation_type_variants() {
        assert_eq!(ConfirmationType::Simple, ConfirmationType::Simple);
        assert_ne!(ConfirmationType::Simple, ConfirmationType::UnsavedChanges);
    }

    #[test]
    fn test_cleanup_result_default() {
        let result = CleanupResult::default();
        assert!(result.deleted_files.is_empty());
        assert!(result.unwatched_paths.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_editor_creation_config_default() {
        let config = EditorCreationConfig::default();
        assert!(config.file_path.is_none());
        assert_eq!(config.tab_size, 0);
        assert!(!config.word_wrap);
    }

    #[test]
    fn test_terminal_creation_config_default() {
        let config = TerminalCreationConfig::default();
        assert!(config.cwd.is_none());
        assert_eq!(config.height, 0);
        assert_eq!(config.width, 0);
    }

    #[test]
    fn test_file_manager_creation_config_default() {
        let config = FileManagerCreationConfig::default();
        assert!(config.working_dir.is_none());
        assert!(!config.show_hidden);
    }

    #[test]
    fn test_panel_creation_config_from_panel_type() {
        let editor_type = PanelType::Editor {
            file_path: Some(PathBuf::from("/test.txt")),
        };
        let config: PanelCreationConfig = editor_type.into();
        assert!(matches!(config, PanelCreationConfig::Editor(_)));

        let terminal_type = PanelType::Terminal {
            cwd: Some(PathBuf::from("/home")),
        };
        let config: PanelCreationConfig = terminal_type.into();
        assert!(matches!(config, PanelCreationConfig::Terminal(_)));

        let fm_type = PanelType::FileManager {
            working_dir: Some(PathBuf::from("/tmp")),
        };
        let config: PanelCreationConfig = fm_type.into();
        assert!(matches!(config, PanelCreationConfig::FileManager(_)));

        let log_type = PanelType::LogViewer;
        let config: PanelCreationConfig = log_type.into();
        assert!(matches!(config, PanelCreationConfig::LogViewer));

        let welcome_type = PanelType::Welcome;
        let config: PanelCreationConfig = welcome_type.into();
        assert!(matches!(config, PanelCreationConfig::Welcome));
    }

    #[test]
    fn test_panel_info() {
        let info = PanelInfo {
            type_name: "Editor".to_string(),
            title: "test.rs".to_string(),
            working_dir: Some(PathBuf::from("/project")),
            has_unsaved_changes: true,
            is_singleton: false,
        };

        assert_eq!(info.type_name, "Editor");
        assert_eq!(info.title, "test.rs");
        assert!(info.has_unsaved_changes);
        assert!(!info.is_singleton);
    }

    #[test]
    fn test_navigation_direction_variants() {
        assert_eq!(
            NavigationDirection::PrevGroup,
            NavigationDirection::PrevGroup
        );
        assert_ne!(
            NavigationDirection::PrevGroup,
            NavigationDirection::NextGroup
        );
    }

    #[test]
    fn test_navigation_result_variants() {
        assert_eq!(NavigationResult::Success, NavigationResult::Success);
        assert_ne!(NavigationResult::Success, NavigationResult::AtBoundary);
        assert_ne!(NavigationResult::AtBoundary, NavigationResult::NoPanels);
    }
}
