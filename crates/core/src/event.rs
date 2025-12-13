//! Event types for termide application.
//!
//! This module provides:
//! - `Event` - Application-level events (keyboard, mouse, resize)
//! - `EventHandler` - Polling for terminal events
//! - `PanelEvent` - Events emitted by panels to communicate with the application

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, KeyEventKind, MouseEvent};

/// Application event
#[derive(Debug, Clone)]
pub enum Event {
    /// Keyboard event
    Key(KeyEvent),
    /// Mouse event
    Mouse(MouseEvent),
    /// Terminal resize event
    Resize(u16, u16),
    /// Tick event (for animations and periodic updates)
    Tick,
    /// Terminal focus lost event
    FocusLost,
    /// Terminal focus gained event
    FocusGained,
}

/// Event handler for polling terminal events
pub struct EventHandler {
    tick_rate: Duration,
}

impl EventHandler {
    /// Create new event handler with specified tick rate
    pub fn new(tick_rate: Duration) -> Self {
        Self { tick_rate }
    }

    /// Wait for next event
    pub fn next(&self) -> Result<Event> {
        if event::poll(self.tick_rate)? {
            match event::read()? {
                // With kitty keyboard protocol, we receive Press, Release, and Repeat events.
                // Only handle Press events to avoid duplicate actions.
                CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => Ok(Event::Key(key)),
                CrosstermEvent::Key(_) => Ok(Event::Tick), // Ignore Release and Repeat
                CrosstermEvent::Mouse(mouse) => Ok(Event::Mouse(mouse)),
                CrosstermEvent::Resize(width, height) => Ok(Event::Resize(width, height)),
                CrosstermEvent::FocusLost => Ok(Event::FocusLost),
                CrosstermEvent::FocusGained => Ok(Event::FocusGained),
                _ => Ok(Event::Tick),
            }
        } else {
            Ok(Event::Tick)
        }
    }
}

/// Events emitted by panels to communicate with the application.
#[derive(Debug, Clone)]
pub enum PanelEvent {
    // === General events ===
    /// Request a UI redraw
    NeedsRedraw,

    /// Request application quit
    Quit,

    // === File operations ===
    /// Open a file in the editor
    OpenFile(PathBuf),

    /// Save file to disk
    SaveFile(PathBuf),

    /// Close current file/panel
    CloseFile,

    /// Request close panel (with confirmation if needed)
    ClosePanel,

    // === Navigation ===
    /// Navigate file manager to path
    NavigateTo(PathBuf),

    /// Go to specific line in editor
    GotoLine(usize),

    // === Modal dialogs ===
    /// Show informational message
    ShowMessage(String),

    /// Show error message
    ShowError(String),

    /// Show confirmation dialog
    ShowConfirm {
        message: String,
        on_confirm: ConfirmAction,
    },

    /// Show input dialog
    ShowInput {
        prompt: String,
        initial_value: String,
        on_submit: InputAction,
    },

    /// Show selection dialog
    ShowSelect {
        title: String,
        options: Vec<String>,
        on_select: SelectAction,
    },

    /// Show search modal
    ShowSearch { initial_query: Option<String> },

    /// Show search & replace modal
    ShowReplace {
        find: Option<String>,
        replace: Option<String>,
    },

    /// Show file conflict resolution modal
    ShowConflict {
        source: PathBuf,
        destination: PathBuf,
        remaining: usize,
    },

    // === Status bar ===
    /// Set status bar message
    SetStatusMessage { message: String, is_error: bool },

    /// Clear status bar message
    ClearStatus,

    // === File watcher registration ===
    /// Register path for watching
    WatchPath(PathBuf),

    /// Unregister path from watching
    UnwatchPath(PathBuf),

    // === Git integration ===
    /// Request git status refresh for path
    RefreshGitStatus(PathBuf),

    // === Clipboard ===
    /// Copy text to clipboard
    CopyToClipboard(String),

    /// Request paste from clipboard
    RequestPaste,

    // === Panel management ===
    /// Request focus on specific panel by name
    FocusPanel(String),

    /// Request panel split
    SplitPanel {
        direction: SplitDirection,
        panel_name: String,
    },

    /// Request next panel focus
    NextPanel,

    /// Request previous panel focus
    PrevPanel,
}

/// Confirmation dialog actions.
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Delete file at path
    DeleteFile(PathBuf),

    /// Delete multiple paths
    DeletePaths(Vec<PathBuf>),

    /// Delete directory at path
    DeleteDirectory(PathBuf),

    /// Discard unsaved changes
    DiscardChanges(PathBuf),

    /// Close panel without saving
    CloseWithoutSaving,

    /// Quit application
    QuitApplication,

    /// Overwrite existing file
    OverwriteFile {
        source: PathBuf,
        destination: PathBuf,
    },
}

/// Input dialog actions.
#[derive(Debug, Clone)]
pub enum InputAction {
    /// Rename file
    RenameFile { from: PathBuf },

    /// Create new file in directory
    CreateFile { in_dir: PathBuf },

    /// Create new directory
    CreateDirectory { in_dir: PathBuf },

    /// Search in file
    SearchInFile,

    /// Search and replace
    SearchReplace,

    /// Go to line number
    GotoLine,

    /// Save file as (new name)
    SaveFileAs { directory: PathBuf },

    /// Copy files to destination
    CopyTo { sources: Vec<PathBuf> },

    /// Move files to destination
    MoveTo { sources: Vec<PathBuf> },
}

/// Selection dialog actions.
#[derive(Debug, Clone)]
pub enum SelectAction {
    /// Select theme
    SelectTheme,

    /// Select language
    SelectLanguage,

    /// Select encoding
    SelectEncoding,

    /// Close editor with save/discard/cancel choice
    CloseEditorChoice,

    /// Custom selection action
    Custom(String),
}

/// File conflict resolution options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Overwrite the destination file
    Overwrite,

    /// Skip this file
    Skip,

    /// Rename the file
    Rename,

    /// Overwrite all remaining files
    OverwriteAll,

    /// Skip all remaining files
    SkipAll,

    /// Cancel the entire operation
    Cancel,
}

/// Direction for panel splits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}
