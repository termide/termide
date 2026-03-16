//! Event types for termide application.
//!
//! This module provides:
//! - `Event` - Application-level events (keyboard, mouse, resize)
//! - `EventHandler` - Polling for terminal events
//! - `PanelEvent` - Events emitted by panels to communicate with the application

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent,
    MouseEventKind,
};

/// Application event
#[derive(Debug, Clone)]
pub enum Event {
    /// Keyboard event
    Key(KeyEvent),
    /// Mouse event
    Mouse(MouseEvent),
    /// Coalesced mouse scroll events (delta: positive=down, negative=up)
    MouseScrollCoalesced {
        /// Original mouse event (for coordinates and modifiers)
        event: MouseEvent,
        /// Combined scroll delta (positive=down, negative=up)
        delta: i32,
    },
    /// Terminal resize event
    Resize(u16, u16),
    /// Tick event (for animations and periodic updates)
    Tick,
    /// Terminal focus lost event
    FocusLost,
    /// Terminal focus gained event
    FocusGained,
    /// Paste event (bracketed paste from terminal)
    Paste(String),
}

/// Event handler for polling terminal events
pub struct EventHandler {
    tick_rate: Duration,
    /// Events read during coalescing but not yet consumed
    pending_events: RefCell<VecDeque<Event>>,
}

impl EventHandler {
    /// Create new event handler with specified tick rate
    pub fn new(tick_rate: Duration) -> Self {
        Self {
            tick_rate,
            pending_events: RefCell::new(VecDeque::new()),
        }
    }

    /// Update the tick rate (for adaptive idle/active polling)
    pub fn set_tick_rate(&mut self, rate: Duration) {
        self.tick_rate = rate;
    }

    /// Wait for next event
    pub fn next(&self) -> Result<Event> {
        // Check pending events first (from previous coalescing)
        if let Some(event) = self.pending_events.borrow_mut().pop_front() {
            return Ok(event);
        }

        if event::poll(self.tick_rate)? {
            match event::read()? {
                // With kitty keyboard protocol, we receive Press, Release, and Repeat events.
                // Only handle Press events to avoid duplicate actions.
                CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    self.try_coalesce_paste(key)
                }
                CrosstermEvent::Key(_) => Ok(Event::Tick), // Ignore Release and Repeat
                CrosstermEvent::Mouse(mouse)
                    if matches!(
                        mouse.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    ) =>
                {
                    self.coalesce_scroll_events(mouse)
                }
                CrosstermEvent::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Drag(_)) => {
                    self.coalesce_drag_events(mouse)
                }
                CrosstermEvent::Mouse(mouse) => Ok(Event::Mouse(mouse)),
                CrosstermEvent::Resize(width, height) => Ok(Event::Resize(width, height)),
                CrosstermEvent::FocusLost => Ok(Event::FocusLost),
                CrosstermEvent::FocusGained => Ok(Event::FocusGained),
                CrosstermEvent::Paste(text) => Ok(Event::Paste(text)),
            }
        } else {
            Ok(Event::Tick)
        }
    }

    /// Coalesce multiple scroll events into a single MouseScrollCoalesced event.
    /// This significantly reduces render cycles during fast scrolling.
    fn coalesce_scroll_events(&self, first: MouseEvent) -> Result<Event> {
        let mut delta: i32 = match first.kind {
            MouseEventKind::ScrollDown => 1,
            MouseEventKind::ScrollUp => -1,
            _ => unreachable!(),
        };

        let (col, row) = (first.column, first.row);

        // Drain queue with zero timeout to collect pending scroll events
        while event::poll(Duration::ZERO)? {
            let raw = event::read()?;
            match &raw {
                CrosstermEvent::Mouse(m)
                    if m.column == col
                        && m.row == row
                        && matches!(
                            m.kind,
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                        ) =>
                {
                    delta += match m.kind {
                        MouseEventKind::ScrollDown => 1,
                        MouseEventKind::ScrollUp => -1,
                        _ => 0,
                    };
                }
                _ => {
                    // Queue non-scroll event for later processing
                    if let Some(ev) = self.convert_crossterm_event(raw) {
                        self.pending_events.borrow_mut().push_back(ev);
                    }
                    break;
                }
            }
        }

        // If scrolls cancelled out, return Tick instead
        if delta == 0 {
            return Ok(Event::Tick);
        }

        Ok(Event::MouseScrollCoalesced {
            event: first,
            delta,
        })
    }

    /// Coalesce multiple drag events into a single event with the latest position.
    /// This prevents processing dozens of intermediate drag positions when the
    /// user moves the mouse quickly, significantly reducing lag over SSH.
    fn coalesce_drag_events(&self, first: MouseEvent) -> Result<Event> {
        let mut latest = first;

        // Drain queue with zero timeout to collect pending drag events
        while event::poll(Duration::ZERO)? {
            let raw = event::read()?;
            match &raw {
                CrosstermEvent::Mouse(m) if matches!(m.kind, MouseEventKind::Drag(_)) => {
                    latest = *m;
                }
                _ => {
                    // Queue non-drag event for later processing
                    if let Some(ev) = self.convert_crossterm_event(raw) {
                        self.pending_events.borrow_mut().push_back(ev);
                    }
                    break;
                }
            }
        }

        Ok(Event::Mouse(latest))
    }

    /// On Windows, pasted text arrives as individual Key events because the console
    /// input API doesn't support bracketed paste. Detect by waiting a few ms after
    /// each character for more input — paste fills the buffer rapidly while typing
    /// has natural gaps. The 5ms wait is imperceptible for typing but enough for
    /// Windows to deliver the full paste buffer.
    fn try_coalesce_paste(&self, first_key: KeyEvent) -> Result<Event> {
        // Only coalesce on Windows - other platforms get native Event::Paste
        if !cfg!(windows) {
            return Ok(Event::Key(first_key));
        }

        // Only coalesce plain character events (Shift is OK for uppercase)
        if first_key.modifiers.contains(KeyModifiers::CONTROL)
            || first_key.modifiers.contains(KeyModifiers::ALT)
            || first_key.modifiers.contains(KeyModifiers::SUPER)
        {
            return Ok(Event::Key(first_key));
        }

        // Must start with a printable character
        let KeyCode::Char(first_ch) = first_key.code else {
            return Ok(Event::Key(first_key));
        };

        // Wait briefly to see if more chars arrive (paste detection).
        // Normal typing: no events within 5ms → return Key immediately.
        // Paste: Windows fills console buffer → events arrive within 5ms.
        if !event::poll(Duration::from_millis(5))? {
            return Ok(Event::Key(first_key));
        }

        let mut text = String::new();
        text.push(first_ch);

        // Paste detected — drain all available chars, then keep waiting
        // for more batches (Windows may deliver paste in chunks).
        loop {
            let before = text.len();
            self.collect_paste_chars(&mut text)?;
            // Wait for next batch; stop when no more arrive
            if text.len() == before || !event::poll(Duration::from_millis(5))? {
                break;
            }
        }

        Ok(Event::Paste(text))
    }

    /// Drain character-like events from the input buffer with zero timeout.
    fn collect_paste_chars(&self, text: &mut String) -> Result<()> {
        while event::poll(Duration::ZERO)? {
            let raw = event::read()?;
            match &raw {
                CrosstermEvent::Key(key)
                    if key.kind == KeyEventKind::Press
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT)
                        && !key.modifiers.contains(KeyModifiers::SUPER) =>
                {
                    match key.code {
                        KeyCode::Char(c) => text.push(c),
                        KeyCode::Enter => text.push('\r'),
                        KeyCode::Tab => text.push('\t'),
                        _ => {
                            if let Some(ev) = self.convert_crossterm_event(raw) {
                                self.pending_events.borrow_mut().push_back(ev);
                            }
                            return Ok(());
                        }
                    }
                }
                CrosstermEvent::Key(_) => continue,
                _ => {
                    if let Some(ev) = self.convert_crossterm_event(raw) {
                        self.pending_events.borrow_mut().push_back(ev);
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Convert crossterm event to our Event type.
    fn convert_crossterm_event(&self, raw: CrosstermEvent) -> Option<Event> {
        match raw {
            CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => Some(Event::Key(key)),
            CrosstermEvent::Key(_) => None, // Ignore Release and Repeat
            CrosstermEvent::Mouse(mouse) => Some(Event::Mouse(mouse)),
            CrosstermEvent::Resize(width, height) => Some(Event::Resize(width, height)),
            CrosstermEvent::FocusLost => Some(Event::FocusLost),
            CrosstermEvent::FocusGained => Some(Event::FocusGained),
            CrosstermEvent::Paste(text) => Some(Event::Paste(text)),
        }
    }
}

/// Type of git operation to execute in background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitOperationType {
    Push,
    Pull,
    Fetch,
}

/// Search mode for the unified search modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Live text search in editor/terminal
    Text,
    /// File search by glob in file manager
    FileGlob,
    /// File mask + content regex in file manager
    Content,
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
    /// Open a file in the editor (editable mode)
    OpenFile(PathBuf),

    /// Open a file in view-only mode (read-only)
    ViewFile(PathBuf),

    /// Open a file at specific location (for go-to-definition)
    OpenFileAt {
        path: PathBuf,
        line: usize,
        column: usize,
    },

    /// Execute file in a new terminal
    ExecuteFile(PathBuf),

    /// Run command in a new terminal
    RunCommand {
        command: String,
        cwd: Option<PathBuf>,
    },

    /// Preview media file (raster image) using native graphics or xdg-open
    PreviewMedia(PathBuf),

    /// Open file with system default application (xdg-open)
    OpenExternal(PathBuf),

    /// Open a remote file via VFS (URL format: "sftp://user@host/path")
    OpenRemoteFile(String),

    /// Save file to disk
    SaveFile(PathBuf),

    /// Close current file/panel
    CloseFile,

    /// Request close panel (with confirmation if needed)
    ClosePanel,

    // === Navigation ===
    /// Navigate file manager to path
    NavigateTo(PathBuf),

    /// Open path in new file manager panel, optionally selecting a file
    OpenPath {
        path: PathBuf,
        select_file: Option<std::ffi::OsString>,
    },

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
    ShowSearch {
        mode: SearchMode,
        initial_query: Option<String>,
    },

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

    /// Execute git operation (push/pull) in background
    GitOperation {
        operation: GitOperationType,
        repo_path: PathBuf,
    },

    /// Cancel current git operation (kill process)
    CancelGitOperation,

    /// Open git diff panel for repository
    /// If commit_hash is Some, shows diff for that commit; otherwise shows working directory changes
    OpenGitDiff {
        repo_path: PathBuf,
        commit_hash: Option<String>,
    },

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

    /// Open diagnostics panel (e.g., when clicking on diagnostic virtual line)
    OpenDiagnosticsPanel,

    /// Vim mode panel navigation (Ctrl+w h/j/k/l)
    VimPanelNavigation {
        /// Direction to navigate
        direction: VimPanelDirection,
    },

    // === Operations panel ===
    /// Toggle pause/resume for a file operation
    ToggleOperationPause(termide_file_ops::OperationId),

    /// Cancel a file operation
    CancelOperation(termide_file_ops::OperationId),

    /// Open or focus the operations panel
    OpenOperationsPanel,

    /// Open or focus the outline panel
    OpenOutlinePanel,
}

/// Direction for Vim panel navigation (Ctrl+w h/j/k/l).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimPanelDirection {
    /// Navigate left (Ctrl+w h)
    Left,
    /// Navigate down (Ctrl+w j)
    Down,
    /// Navigate up (Ctrl+w k)
    Up,
    /// Navigate right (Ctrl+w l)
    Right,
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

    /// Rename all remaining files with auto-generated names
    RenameAll,

    /// Cancel the entire operation
    Cancel,
}

/// Direction for panel splits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}
