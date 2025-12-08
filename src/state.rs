use crate::config::Config;
use crate::git::{GitStatusUpdate, GitWatcher};
use crate::theme::Theme;
use crate::ui::modal::{
    ConfirmModal, ConflictModal, InfoModal, InputModal, OverwriteModal, SearchModal, SelectModal,
};
use std::path::PathBuf;
use std::sync::mpsc;

/// Message about background directory size calculation result
#[derive(Debug)]
pub struct DirSizeResult {
    pub size: u64,
}

/// Batch operation type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BatchOperationType {
    Copy,
    Move,
}

/// Automatic conflict resolution mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConflictMode {
    /// Ask each time
    Ask,
    /// Automatically overwrite all
    OverwriteAll,
    /// Automatically skip all
    SkipAll,
}

/// Batch file operation with conflict support
#[derive(Debug, Clone)]
pub struct BatchOperation {
    /// Operation type
    pub operation_type: BatchOperationType,
    /// List of files to process
    pub sources: Vec<PathBuf>,
    /// Target directory
    pub destination: PathBuf,
    /// Current index of file being processed
    pub current_index: usize,
    /// Conflict resolution mode
    pub conflict_mode: ConflictMode,
    /// Rename pattern for RenameAll
    pub rename_pattern: Option<crate::rename_pattern::RenamePattern>,
    /// Counter for $I variable in pattern
    pub rename_counter: usize,
    /// Statistics: successfully processed
    pub success_count: usize,
    /// Statistics: errors
    pub error_count: usize,
    /// Statistics: skipped
    pub skipped_count: usize,
}

impl BatchOperation {
    /// Create new batch operation
    pub fn new(
        operation_type: BatchOperationType,
        sources: Vec<PathBuf>,
        destination: PathBuf,
    ) -> Self {
        Self {
            operation_type,
            sources,
            destination,
            current_index: 0,
            conflict_mode: ConflictMode::Ask,
            rename_pattern: None,
            rename_counter: 1,
            success_count: 0,
            error_count: 0,
            skipped_count: 0,
        }
    }

    /// Set rename pattern
    pub fn set_rename_pattern(&mut self, pattern: crate::rename_pattern::RenamePattern) {
        self.rename_pattern = Some(pattern);
    }

    /// Get and increment rename counter
    pub fn get_and_increment_rename_counter(&mut self) -> usize {
        let counter = self.rename_counter;
        self.rename_counter += 1;
        counter
    }

    /// Get current file being processed
    pub fn current_source(&self) -> Option<&PathBuf> {
        self.sources.get(self.current_index)
    }

    /// Check if operation is complete
    pub fn is_complete(&self) -> bool {
        self.current_index >= self.sources.len()
    }

    /// Advance to next file
    pub fn advance(&mut self) {
        self.current_index += 1;
    }

    /// Total number of files
    pub fn total_count(&self) -> usize {
        self.sources.len()
    }

    /// Set conflict resolution mode
    pub fn set_conflict_mode(&mut self, mode: ConflictMode) {
        self.conflict_mode = mode;
    }

    /// Increment success counter
    pub fn increment_success(&mut self) {
        self.success_count += 1;
    }

    /// Increment error counter
    pub fn increment_error(&mut self) {
        self.error_count += 1;
    }

    /// Increment skipped counter
    pub fn increment_skipped(&mut self) {
        self.skipped_count += 1;
    }
}

/// Layout mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Single panel mode (width 1-80)
    Single,
    /// Multi-panel mode (width > 100)
    MultiPanel,
}

/// Active modal window
#[derive(Debug)]
pub enum ActiveModal {
    /// Confirmation modal
    Confirm(Box<ConfirmModal>),
    /// Text input modal
    Input(Box<InputModal>),
    /// Selection modal
    Select(Box<SelectModal>),
    /// File overwrite modal
    #[allow(dead_code)]
    Overwrite(Box<OverwriteModal>),
    /// File conflict resolution modal
    Conflict(Box<ConflictModal>),
    /// Information modal
    Info(Box<InfoModal>),
    /// Rename pattern input modal
    RenamePattern(Box<crate::ui::modal::RenamePatternModal>),
    /// Editable select modal (combobox with editable input)
    EditableSelect(Box<crate::ui::modal::EditableSelectModal>),
    /// Interactive search modal
    Search(Box<SearchModal>),
    /// Interactive replace modal
    Replace(Box<crate::ui::modal::ReplaceModal>),
}

/// Action pending modal result
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// Create new file in specified directory
    CreateFile {
        panel_index: usize,
        directory: PathBuf,
    },
    /// Create new directory in specified directory
    CreateDirectory {
        panel_index: usize,
        directory: PathBuf,
    },
    /// Delete files/directories (one or multiple)
    DeletePath {
        panel_index: usize,
        paths: Vec<PathBuf>,
    },
    /// Copy files/directories (one or multiple)
    CopyPath {
        panel_index: usize,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
    },
    /// Move files/directories (one or multiple)
    MovePath {
        panel_index: usize,
        sources: Vec<PathBuf>,
        target_directory: Option<PathBuf>,
    },
    /// Save unnamed file (Save As)
    SaveFileAs {
        panel_index: usize,
        directory: PathBuf,
    },
    /// Close panel (with confirmation if there are unsaved changes)
    ClosePanel { panel_index: usize },
    /// Close editor with choice: save, don't save, cancel
    CloseEditorWithSave { panel_index: usize },
    /// Close editor with external changes (file changed on disk)
    CloseEditorExternal { panel_index: usize },
    /// Close editor with conflict (local changes + external changes)
    CloseEditorConflict { panel_index: usize },
    /// File overwrite decision when copying/moving
    #[allow(dead_code)]
    OverwriteDecision {
        panel_index: usize,
        source: PathBuf,
        destination: PathBuf,
        is_move: bool, // true for move, false for copy
    },
    /// Batch file operation (copy/move)
    #[allow(dead_code)]
    BatchFileOperation { operation: BatchOperation },
    /// Continue batch operation after conflict resolution
    ContinueBatchOperation { operation: BatchOperation },
    /// Request rename pattern and apply to file
    RenameWithPattern {
        operation: BatchOperation,
        original_name: String,
    },
    /// Text search in editor
    Search,
    /// Text replace in editor
    Replace,
    /// Switch to next panel
    NextPanel,
    /// Switch to previous panel
    PrevPanel,
    /// Quit application (with confirmation if there are unsaved changes)
    QuitApplication,
}

/// Layout information
#[derive(Debug, Clone)]
pub struct LayoutInfo {
    /// Layout mode
    pub mode: LayoutMode,
    /// Number of main panels
    pub main_panels_count: usize,
    /// Width of one main panel
    #[allow(dead_code)]
    pub main_panel_width: u16,
}

/// UI components state
#[derive(Debug, Default)]
pub struct UiState {
    /// Is menu open
    pub menu_open: bool,
    /// Selected menu item (None if menu closed)
    pub selected_menu_item: Option<usize>,
    /// Selected item in dropdown list
    pub selected_dropdown_item: usize,
    /// Status line message (for displaying errors and notifications)
    pub status_message: Option<(String, bool)>, // (message, is_error)
}

/// Terminal state (dimensions)
#[derive(Debug, Clone, Copy)]
pub struct TerminalState {
    /// Terminal width
    pub width: u16,
    /// Terminal height
    pub height: u16,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
        }
    }
}

/// Global application state
#[derive(Debug)]
pub struct AppState {
    /// Should application quit
    pub should_quit: bool,
    /// UI components state
    pub ui: UiState,
    /// Terminal state
    pub terminal: TerminalState,
    /// Current layout mode
    pub layout_mode: LayoutMode,
    /// Current layout information
    pub layout_info: LayoutInfo,
    /// Active modal window
    pub active_modal: Option<ActiveModal>,
    /// Action pending modal result
    pub pending_action: Option<PendingAction>,
    /// Receiver channel for background directory size calculation results
    pub dir_size_receiver: Option<mpsc::Receiver<DirSizeResult>>,
    /// Receiver channel for git status update events
    pub git_watcher_receiver: Option<mpsc::Receiver<GitStatusUpdate>>,
    /// Git watcher instance (kept alive for cleanup)
    pub git_watcher: Option<GitWatcher>,
    /// Receiver channel for filesystem update events
    pub fs_watcher_receiver: Option<mpsc::Receiver<crate::fs_watcher::DirectoryUpdate>>,
    /// Filesystem watcher instance (kept alive for cleanup)
    pub fs_watcher: Option<crate::fs_watcher::FileSystemWatcher>,
    /// Current theme
    pub theme: &'static Theme,
    /// Application configuration
    pub config: Config,
    /// System resource monitor (CPU, RAM)
    pub system_monitor: crate::system_monitor::SystemMonitor,
    /// Last time system resources were updated
    pub last_resource_update: std::time::Instant,
    /// Last time session was saved (for debouncing autosave)
    pub last_session_save: Option<std::time::Instant>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// Create new application state, loading config from file
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_else(|e| {
            eprintln!("Warning: Could not load config: {}. Using defaults.", e);
            Config::default()
        });
        let theme = Theme::get_by_name(&config.theme);
        Self::with_config_and_theme(config, theme)
    }

    /// Create new application state with given config and theme
    pub fn with_config_and_theme(config: Config, theme: &'static Theme) -> Self {
        let layout_info = LayoutInfo {
            mode: LayoutMode::Single,
            main_panels_count: 1,
            main_panel_width: crate::constants::DEFAULT_MAIN_PANEL_WIDTH,
        };

        // Note: Logger is initialized in App::new() after project_root is known,
        // so logs go to session directory

        Self {
            should_quit: false,
            ui: UiState::default(),
            terminal: TerminalState::default(),
            layout_mode: LayoutMode::Single,
            layout_info,
            active_modal: None,
            pending_action: None,
            dir_size_receiver: None,
            git_watcher_receiver: None,
            git_watcher: None,
            fs_watcher_receiver: None,
            fs_watcher: None,
            theme,
            config,
            system_monitor: crate::system_monitor::SystemMonitor::new(),
            last_resource_update: std::time::Instant::now(),
            last_session_save: None,
        }
    }

    /// Set new theme and update config
    pub fn set_theme(&mut self, theme_name: &str) {
        self.theme = Theme::get_by_name(theme_name);
        self.config.theme = theme_name.to_string();
    }

    /// Request application quit
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Open menu
    pub fn open_menu(&mut self, menu_index: Option<usize>) {
        self.ui.menu_open = true;
        self.ui.selected_menu_item = menu_index;
        self.ui.selected_dropdown_item = 0;
    }

    /// Close menu
    pub fn close_menu(&mut self) {
        self.ui.menu_open = false;
        self.ui.selected_menu_item = None;
        self.ui.selected_dropdown_item = 0;
    }

    /// Toggle menu
    pub fn toggle_menu(&mut self) {
        if self.ui.menu_open {
            self.close_menu();
        } else {
            self.open_menu(Some(0));
        }
    }

    /// Move to next menu item
    pub fn next_menu_item(&mut self, menu_count: usize) {
        if let Some(current) = self.ui.selected_menu_item {
            self.ui.selected_menu_item = Some((current + 1) % menu_count);
            self.ui.selected_dropdown_item = 0;
        }
    }

    /// Move to previous menu item
    pub fn prev_menu_item(&mut self, menu_count: usize) {
        if let Some(current) = self.ui.selected_menu_item {
            self.ui.selected_menu_item = Some(if current == 0 {
                menu_count - 1
            } else {
                current - 1
            });
            self.ui.selected_dropdown_item = 0;
        }
    }

    /// Update terminal dimensions
    pub fn update_terminal_size(&mut self, width: u16, height: u16) {
        self.terminal.width = width;
        self.terminal.height = height;
        self.layout_info = Self::calculate_layout(width);
        self.layout_mode = self.layout_info.mode;
    }

    /// Calculate layout based on terminal width
    pub fn calculate_layout(width: u16) -> LayoutInfo {
        if width <= crate::constants::MIN_WIDTH_SINGLE_PANEL {
            // Single panel mode
            LayoutInfo {
                mode: LayoutMode::Single,
                main_panels_count: 1,
                main_panel_width: width,
            }
        } else if width <= crate::constants::MIN_WIDTH_MULTI_PANEL {
            // Insufficient width for multi-panel mode
            // Use single panel
            LayoutInfo {
                mode: LayoutMode::Single,
                main_panels_count: 1,
                main_panel_width: width,
            }
        } else {
            // Multi-panel mode
            // Each main panel minimum MIN_MAIN_PANEL_WIDTH characters
            let main_panels_count =
                (width / crate::constants::MIN_MAIN_PANEL_WIDTH).max(1) as usize;
            let main_panel_width = width / main_panels_count as u16;

            LayoutInfo {
                mode: LayoutMode::MultiPanel,
                main_panels_count,
                main_panel_width,
            }
        }
    }

    /// Get recommended layout based on terminal width
    pub fn get_recommended_layout(&self) -> &'static str {
        match self.layout_mode {
            LayoutMode::Single => "Single panel",
            LayoutMode::MultiPanel => {
                match self.layout_info.main_panels_count {
                    1 => "1 panel",
                    2 => "2 panels",
                    3 => "3 panels",
                    4 => "4 panels",
                    5 => "5 panels",
                    6 => "6 panels",
                    7 => "7 panels",
                    8 => "8 panels",
                    9 => "9 panels",
                    _ => "Many panels", // For 10+ panels
                }
            }
        }
    }

    /// Close modal window
    pub fn close_modal(&mut self) {
        self.active_modal = None;
    }

    /// Check if modal window is open
    pub fn has_modal(&self) -> bool {
        self.active_modal.is_some()
    }

    /// Get mutable reference to active modal window
    pub fn get_active_modal_mut(&mut self) -> Option<&mut ActiveModal> {
        self.active_modal.as_mut()
    }

    /// Set pending action and open modal window
    pub fn set_pending_action(&mut self, action: PendingAction, modal: ActiveModal) {
        self.pending_action = Some(action);
        self.active_modal = Some(modal);
    }

    /// Take pending action (take ownership)
    pub fn take_pending_action(&mut self) -> Option<PendingAction> {
        self.pending_action.take()
    }

    /// Set error message
    pub fn set_error(&mut self, message: String) {
        self.ui.status_message = Some((message, true));
    }

    /// Set informational message
    pub fn set_info(&mut self, message: String) {
        self.ui.status_message = Some((message, false));
    }

    /// Clear status message
    pub fn clear_status(&mut self) {
        self.ui.status_message = None;
    }

    /// Create EditorConfig with settings from global config
    pub fn editor_config(&self) -> crate::panels::editor::EditorConfig {
        let mut config = crate::panels::editor::EditorConfig::default();
        config.tab_size = self.config.tab_size;
        config.word_wrap = self.config.word_wrap;
        config
    }

    /// Check if enough time has passed since last session save (debounce check)
    /// Returns true if we should save the session
    pub fn should_save_session(&self) -> bool {
        const DEBOUNCE_DURATION: std::time::Duration = std::time::Duration::from_secs(1);

        match self.last_session_save {
            None => true, // Never saved before
            Some(last_save) => last_save.elapsed() >= DEBOUNCE_DURATION,
        }
    }

    /// Update last session save timestamp
    pub fn update_last_session_save(&mut self) {
        self.last_session_save = Some(std::time::Instant::now());
    }
}
