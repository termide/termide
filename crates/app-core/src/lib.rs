//! Core traits and types for termide app modules.
//!
//! This crate provides the foundational abstractions for building
//! modular app components. It defines:
//!
//! - **Traits**: Standardized interfaces for synchronous operations
//! - **Message Bus**: Types for asynchronous background operations
//! - **Commands**: Explicit state mutation requests
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    termide-app (orchestrator)                │
//! └─────────────────────────────────────────────────────────────┘
//!                               │
//!      Traits (sync)      Message Bus (async)      Commands
//!            ▼                    ▼                    ▼
//! ┌──────────────────────────────────────────────────────────────┐
//! │                    termide-app-core (this crate)              │
//! │  StateManager, ModalManager, PanelProvider, LayoutController │
//! │  Message enum, AppCommand enum                                │
//! └──────────────────────────────────────────────────────────────┘
//! ```

use std::path::{Path, PathBuf};

use anyhow::Result;

// Re-export foundation types from dependent crates
pub use termide_core::{Panel, PanelEvent};
pub use termide_modal::ActiveModal;
pub use termide_state::{PendingAction, UiState};

// ============================================================================
// Synchronous Operation Traits
// ============================================================================

/// State management interface for UI state operations.
///
/// Provides access to UI state and status messages without
/// requiring knowledge of the underlying AppState structure.
pub trait StateManager {
    /// Get immutable reference to UI state.
    fn ui(&self) -> &UiState;

    /// Get mutable reference to UI state.
    fn ui_mut(&mut self) -> &mut UiState;

    /// Set informational status message.
    fn set_info(&mut self, msg: String);

    /// Set error status message.
    fn set_error(&mut self, msg: String);

    /// Clear status message.
    fn clear_status(&mut self);

    /// Check if redraw is needed.
    fn needs_redraw(&self) -> bool;

    /// Set redraw flag.
    fn set_redraw(&mut self, value: bool);
}

/// Modal lifecycle management interface.
///
/// Handles opening, closing, and accessing modal dialogs
/// with their associated pending actions.
pub trait ModalManager {
    /// Get reference to active modal if one is open.
    fn active_modal(&self) -> Option<&ActiveModal>;

    /// Get mutable reference to active modal.
    fn active_modal_mut(&mut self) -> Option<&mut ActiveModal>;

    /// Open a modal with optional pending action.
    fn open_modal(&mut self, modal: ActiveModal, action: Option<PendingAction>);

    /// Close the active modal.
    fn close_modal(&mut self);

    /// Take the pending action (moving it out).
    fn take_pending_action(&mut self) -> Option<PendingAction>;

    /// Check if a modal is currently open.
    fn has_modal(&self) -> bool {
        self.active_modal().is_some()
    }
}

/// Panel access interface for iteration and lookup.
///
/// Provides read and write access to panels without exposing
/// the underlying storage mechanism.
pub trait PanelProvider {
    /// Get reference to the currently active panel.
    fn active_panel(&self) -> Option<&dyn Panel>;

    /// Get mutable reference to the active panel.
    fn active_panel_mut(&mut self) -> Option<&mut Box<dyn Panel>>;

    /// Get the index of the currently active panel group.
    fn active_panel_index(&self) -> Option<usize>;

    /// Get total number of panels.
    fn panel_count(&self) -> usize;

    /// Iterate mutably over all panels.
    fn iter_panels_mut(&mut self) -> Box<dyn Iterator<Item = &mut Box<dyn Panel>> + '_>;
}

/// Layout control interface for panel management.
///
/// Provides operations for adding, removing, and navigating panels
/// within the layout structure.
pub trait LayoutController {
    /// Add a new panel to the layout.
    fn add_panel(&mut self, panel: Box<dyn Panel>);

    /// Close the currently active panel.
    fn close_active(&mut self) -> Result<()>;

    /// Navigate to the next panel group.
    fn next_group(&mut self);

    /// Navigate to the previous panel group.
    fn prev_group(&mut self);

    /// Navigate to the next panel within the current group.
    fn next_in_group(&mut self);

    /// Navigate to the previous panel within the current group.
    fn prev_in_group(&mut self);

    /// Set focus to a specific panel by index.
    fn set_focus(&mut self, index: usize);
}

// ============================================================================
// Message Bus Types (Asynchronous Operations)
// ============================================================================

/// Git status information for a path.
#[derive(Debug, Clone)]
pub struct GitStatus {
    /// Branch name (e.g., "main", "feature/foo")
    pub branch: Option<String>,
    /// Number of modified files
    pub modified_count: usize,
    /// Number of untracked files
    pub untracked_count: usize,
    /// Number of staged files
    pub staged_count: usize,
    /// Whether the repo has uncommitted changes
    pub is_dirty: bool,
}

/// File system change type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// File or directory was created
    Created,
    /// File or directory was modified
    Modified,
    /// File or directory was deleted
    Deleted,
    /// File or directory was renamed
    Renamed,
}

/// Message type for asynchronous background operations.
///
/// These messages are produced by background tasks (git watcher,
/// filesystem watcher, etc.) and consumed by the main event loop.
#[derive(Debug, Clone)]
pub enum Message {
    // === Background task results ===
    /// Git status update for a repository
    GitStatusUpdate {
        /// Repository root path
        path: PathBuf,
        /// Updated git status
        status: GitStatus,
    },

    /// Filesystem update notification
    FsUpdate {
        /// Path that changed
        path: PathBuf,
        /// Type of change
        change: FileChange,
    },

    /// Directory size calculation result
    DirSizeResult {
        /// Directory path
        path: PathBuf,
        /// Calculated size in bytes
        size: u64,
    },

    // === System events ===
    /// Terminal was resized
    TerminalResize {
        /// New width in columns
        width: u16,
        /// New height in rows
        height: u16,
    },

    /// Terminal focus changed
    FocusChange {
        /// Whether the terminal has focus
        focused: bool,
    },

    // === Deferred operations ===
    /// Request to refresh panels for given paths
    RefreshPanels {
        /// Paths that need refreshing
        paths: Vec<PathBuf>,
    },

    /// Request to save current session
    SaveSession,
}

/// Trait for receiving messages from background tasks.
pub trait MessageReceiver {
    /// Poll for new messages (non-blocking).
    ///
    /// Returns all available messages without waiting.
    fn poll_messages(&mut self) -> Vec<Message>;
}

// ============================================================================
// Commands (Explicit State Mutations)
// ============================================================================

/// Navigation direction for panel focus changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Navigate to next panel/group
    Next,
    /// Navigate to previous panel/group
    Prev,
    /// Navigate to specific index
    Index(usize),
}

/// Panel type for creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelType {
    /// File manager panel
    FileManager {
        /// Optional starting directory
        working_dir: Option<PathBuf>,
    },
    /// Editor panel
    Editor {
        /// Optional file to open
        file_path: Option<PathBuf>,
    },
    /// Terminal panel
    Terminal {
        /// Optional starting directory
        cwd: Option<PathBuf>,
    },
    /// Log viewer panel
    LogViewer,
    /// Welcome screen panel
    Welcome,
}

/// Command type for explicit state mutations.
///
/// Commands represent user-initiated or system-initiated actions
/// that modify application state. They are processed by the
/// app orchestrator in a controlled manner.
#[derive(Debug)]
pub enum AppCommand {
    // === Status bar ===
    /// Set status bar message
    SetStatus {
        /// Message text
        message: String,
        /// Whether this is an error message
        is_error: bool,
    },
    /// Clear status bar
    ClearStatus,

    // === Modal operations ===
    /// Open a modal dialog
    OpenModal {
        /// The modal to open
        modal: ActiveModal,
        /// Associated pending action
        action: Option<PendingAction>,
    },
    /// Close the active modal
    CloseModal,

    // === Panel operations ===
    /// Create a new panel
    CreatePanel {
        /// Type of panel to create
        panel_type: PanelType,
    },
    /// Close the active panel
    ClosePanel,

    // === Navigation ===
    /// Navigate between panels/groups
    Navigate {
        /// Direction to navigate
        direction: Direction,
    },

    // === Application lifecycle ===
    /// Request to quit the application
    Quit,
    /// Force quit without confirmation
    ForceQuit,

    // === Session ===
    /// Save current session
    SaveSession,

    // === Panel events ===
    /// Forward a panel event
    PanelEvent {
        /// Panel index to send event to
        panel_index: usize,
        /// The event to forward
        event: PanelEvent,
    },
}

// ============================================================================
// Context Traits (Composite Interfaces)
// ============================================================================

/// Context for event handling operations.
///
/// Combines state management, modal management, and panel access
/// for use in event handlers.
pub trait EventContext: StateManager + ModalManager + PanelProvider {
    /// Get terminal dimensions.
    fn terminal_size(&self) -> (u16, u16);

    /// Check if menu is currently open.
    fn menu_open(&self) -> bool;
}

/// Context for modal result processing.
///
/// Provides access to state and panels for processing
/// modal dialog results.
pub trait ModalContext: StateManager + PanelProvider {
    /// Get mutable access to file operations handler.
    fn file_operations(&mut self) -> &mut dyn FileOperations;
}

/// File operations interface for modal handlers.
///
/// Abstracts filesystem operations for testability.
pub trait FileOperations {
    /// Create a new file.
    fn create_file(&mut self, path: &Path) -> Result<()>;

    /// Create a new directory.
    fn create_directory(&mut self, path: &Path) -> Result<()>;

    /// Delete a path (file or directory).
    fn delete_path(&mut self, path: &Path) -> Result<()>;

    /// Copy a path to destination.
    fn copy_path(&mut self, source: &Path, destination: &Path) -> Result<()>;

    /// Move a path to destination.
    fn move_path(&mut self, source: &Path, destination: &Path) -> Result<()>;

    /// Check if path exists.
    fn path_exists(&self, path: &Path) -> bool;

    /// Check if path is a directory.
    fn is_directory(&self, path: &Path) -> bool;
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_status_default() {
        let status = GitStatus {
            branch: Some("main".to_string()),
            modified_count: 0,
            untracked_count: 0,
            staged_count: 0,
            is_dirty: false,
        };
        assert_eq!(status.branch, Some("main".to_string()));
        assert!(!status.is_dirty);
    }

    #[test]
    fn test_file_change_variants() {
        assert_eq!(FileChange::Created, FileChange::Created);
        assert_ne!(FileChange::Created, FileChange::Modified);
    }

    #[test]
    fn test_message_git_status() {
        let msg = Message::GitStatusUpdate {
            path: PathBuf::from("/repo"),
            status: GitStatus {
                branch: Some("main".to_string()),
                modified_count: 2,
                untracked_count: 0,
                staged_count: 1,
                is_dirty: true,
            },
        };
        if let Message::GitStatusUpdate { path, status } = msg {
            assert_eq!(path, PathBuf::from("/repo"));
            assert_eq!(status.modified_count, 2);
            assert!(status.is_dirty);
        } else {
            panic!("Expected GitStatusUpdate message");
        }
    }

    #[test]
    fn test_message_fs_update() {
        let msg = Message::FsUpdate {
            path: PathBuf::from("/file.txt"),
            change: FileChange::Modified,
        };
        if let Message::FsUpdate { path, change } = msg {
            assert_eq!(path, PathBuf::from("/file.txt"));
            assert_eq!(change, FileChange::Modified);
        } else {
            panic!("Expected FsUpdate message");
        }
    }

    #[test]
    fn test_message_terminal_resize() {
        let msg = Message::TerminalResize {
            width: 120,
            height: 40,
        };
        if let Message::TerminalResize { width, height } = msg {
            assert_eq!(width, 120);
            assert_eq!(height, 40);
        } else {
            panic!("Expected TerminalResize message");
        }
    }

    #[test]
    fn test_direction_variants() {
        assert_eq!(Direction::Next, Direction::Next);
        assert_ne!(Direction::Next, Direction::Prev);
        assert_eq!(Direction::Index(0), Direction::Index(0));
        assert_ne!(Direction::Index(0), Direction::Index(1));
    }

    #[test]
    fn test_panel_type_file_manager() {
        let pt = PanelType::FileManager {
            working_dir: Some(PathBuf::from("/home")),
        };
        if let PanelType::FileManager { working_dir } = pt {
            assert_eq!(working_dir, Some(PathBuf::from("/home")));
        } else {
            panic!("Expected FileManager");
        }
    }

    #[test]
    fn test_panel_type_editor() {
        let pt = PanelType::Editor {
            file_path: Some(PathBuf::from("/file.rs")),
        };
        if let PanelType::Editor { file_path } = pt {
            assert_eq!(file_path, Some(PathBuf::from("/file.rs")));
        } else {
            panic!("Expected Editor");
        }
    }

    #[test]
    fn test_app_command_set_status() {
        let cmd = AppCommand::SetStatus {
            message: "Hello".to_string(),
            is_error: false,
        };
        if let AppCommand::SetStatus { message, is_error } = cmd {
            assert_eq!(message, "Hello");
            assert!(!is_error);
        } else {
            panic!("Expected SetStatus");
        }
    }

    #[test]
    fn test_app_command_navigate() {
        let cmd = AppCommand::Navigate {
            direction: Direction::Next,
        };
        if let AppCommand::Navigate { direction } = cmd {
            assert_eq!(direction, Direction::Next);
        } else {
            panic!("Expected Navigate");
        }
    }

    #[test]
    fn test_app_command_create_panel() {
        let cmd = AppCommand::CreatePanel {
            panel_type: PanelType::Terminal { cwd: None },
        };
        if let AppCommand::CreatePanel { panel_type } = cmd {
            assert_eq!(panel_type, PanelType::Terminal { cwd: None });
        } else {
            panic!("Expected CreatePanel");
        }
    }
}
