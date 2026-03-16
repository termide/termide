//! Application state and types.
//!
//! Re-exports pure types from termide-state crate and defines
//! complex types that depend on other application modules.
//!
//! Implements core traits from termide-app-core for standardized
//! state management and modal handling.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use termide_config::{BookmarksConfig, Config};
use termide_file_ops::{
    BackgroundOperationSummary, OperationEvent, OperationManager, OperationRequest,
};
use termide_lsp::{LspConfig, LspManager, LspServerConfig};
use termide_panel_editor::EditorConfig;
use termide_system_monitor::SystemMonitor;
use termide_theme::Theme;
use termide_vfs::{VfsManager, VfsPath};
use termide_watcher::UnifiedWatcher;

// Import core traits
use termide_app_core::{ModalManager, StateManager};

// Re-export pure types from state crate
pub use termide_state::{
    ActiveOperation, BatchOperation, BatchOperationType, ConflictMode, DirSizeResult, LayoutInfo,
    LayoutMode, OperationProgress, OperationType, PendingAction, RenamePattern, SpeedTracker,
    SubmenuState, TerminalState, UiState,
};

// Re-export ActiveModal from modal crate
pub use termide_modal::ActiveModal;

/// Which resource modal is open (for auto-refresh in tick handler).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceModalKind {
    Cpu,
    Ram,
}

/// Result of background git operation (push/pull)
#[derive(Debug)]
pub struct GitOperationResult {
    /// Operation type: "push" or "pull"
    pub operation: String,
    /// Whether the operation succeeded
    pub success: bool,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

/// Handle for background git operation (allows cancellation)
pub struct GitOperationHandle {
    /// Receiver for operation result
    pub receiver: mpsc::Receiver<GitOperationResult>,
    /// Process ID for cancellation
    pub pid: u32,
    /// Operation type: "push" or "pull"
    pub operation: String,
    /// When the operation was started (for timeout detection)
    pub started_at: std::time::Instant,
}

impl std::fmt::Debug for GitOperationHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitOperationHandle")
            .field("pid", &self.pid)
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

/// Result of background script operation (.report. scripts)
#[derive(Debug)]
pub struct ScriptOperationResult {
    /// Script display name
    pub script_name: String,
    /// Whether the script succeeded (exit code 0)
    pub success: bool,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

/// Handle for background script operation
pub struct ScriptOperationHandle {
    /// Receiver for operation result
    pub receiver: mpsc::Receiver<ScriptOperationResult>,
    /// Script display name
    pub script_name: String,
}

impl std::fmt::Debug for ScriptOperationHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptOperationHandle")
            .field("script_name", &self.script_name)
            .finish_non_exhaustive()
    }
}

/// Pending editor download — tracks a download operation that should open an editor on completion.
/// Used when opening remote files: the download runs via OperationManager, and on completion
/// the editor is opened with the downloaded temp file.
pub struct PendingEditorDownload {
    /// OperationManager operation ID for the download
    pub operation_id: termide_file_ops::OperationId,
    /// Remote path being downloaded
    pub remote_path: VfsPath,
    /// Local temp path where file is being downloaded
    pub temp_path: PathBuf,
    /// Editor config for opening the file
    pub config: termide_panel_editor::EditorConfig,
    /// VFS manager reference for opening the editor
    pub vfs_manager: Arc<VfsManager>,
}

impl std::fmt::Debug for PendingEditorDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingEditorDownload")
            .field("operation_id", &self.operation_id)
            .field("remote_path", &self.remote_path.to_url_string())
            .field("temp_path", &self.temp_path)
            .finish_non_exhaustive()
    }
}

/// Pending remote delete for move operations (delete source after download completes).
///
/// When downloading from remote with is_move=true, we need to delete the source
/// after the download succeeds. This stores the VFS info needed for that deletion.
pub struct PendingRemoteDelete {
    /// VFS source path to delete
    pub vfs_source: termide_vfs::VfsPath,
    /// VFS manager for the delete operation
    pub vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
}

impl std::fmt::Debug for PendingRemoteDelete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingRemoteDelete")
            .field("vfs_source", &self.vfs_source.to_url_string())
            .finish_non_exhaustive()
    }
}

/// Pending batch upload state (tracks remaining files after current upload completes).
///
/// When uploading multiple files via OperationManager, this tracks the batch state
/// so we can continue with the next file after each upload completes.
pub struct PendingBatchUpload {
    /// All source files to upload
    pub all_sources: Vec<PathBuf>,
    /// Current file index in the batch
    pub current_index: usize,
    /// Remote destination base URL (directory)
    pub dest_base_url: String,
    /// VFS manager for the upload
    pub vfs_manager: std::sync::Arc<termide_vfs::VfsManager>,
    /// Whether this is a move operation (delete source after upload)
    pub is_move: bool,
    /// Current source path being uploaded (for move delete)
    pub current_source: PathBuf,
}

impl std::fmt::Debug for PendingBatchUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingBatchUpload")
            .field("current_index", &self.current_index)
            .field("total_files", &self.all_sources.len())
            .field("dest_base_url", &self.dest_base_url)
            .field("is_move", &self.is_move)
            .finish_non_exhaustive()
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
    /// Handle for background git operation (allows cancellation)
    pub git_operation_handle: Option<GitOperationHandle>,
    /// Handle for background script operation (.report. scripts)
    pub script_operation_handle: Option<ScriptOperationHandle>,
    /// Pending editor download via OperationManager (replaces download_operation for editor opens)
    pub pending_editor_download: Option<PendingEditorDownload>,
    /// Pending batch upload state (for OperationManager-based uploads)
    pub pending_batch_upload: Option<PendingBatchUpload>,
    /// Pending remote delete for move operations (delete source after download)
    pub pending_remote_delete: Option<PendingRemoteDelete>,
    /// Close editor after current upload completes (for "save and close" flow).
    /// Stores the file path of the editor to close (to find the correct panel).
    pub close_editor_after_upload: Option<PathBuf>,
    /// Skip file manager refresh after upload (for editor saves - file already exists)
    pub skip_refresh_after_upload: bool,
    /// Unified watcher for filesystem and git changes
    pub watcher: Option<UnifiedWatcher>,
    /// Current theme
    pub theme: &'static Theme,
    /// Application configuration
    pub config: Config,
    /// System resource monitor (CPU, RAM)
    pub system_monitor: SystemMonitor,
    /// Last time system resources were updated
    pub last_resource_update: std::time::Instant,
    /// Currently open resource modal (for auto-refresh in tick)
    pub resource_modal_kind: Option<ResourceModalKind>,
    /// Last time resource modal was refreshed
    pub last_resource_modal_refresh: Option<std::time::Instant>,
    /// Last time session was saved (for debouncing autosave)
    pub last_session_save: Option<std::time::Instant>,
    /// Flag indicating UI needs to be redrawn (for CPU optimization)
    pub needs_redraw: bool,
    /// Last time spinner was updated (for throttling spinner animation)
    pub last_spinner_update: Option<std::time::Instant>,
    /// Last time LSP loading spinner was redrawn (for throttling to 125ms/8 FPS)
    pub last_lsp_loading_redraw: Option<std::time::Instant>,
    /// Last time git operation spinner was updated (for throttling to 125ms/8 FPS)
    pub last_git_spinner_update: Option<std::time::Instant>,
    /// LSP manager for language server integration
    pub lsp_manager: Option<LspManager>,
    /// All diagnostics from LSP servers, keyed by file path
    pub all_diagnostics: HashMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// User bookmarks
    pub bookmarks: BookmarksConfig,
    /// Unified operation manager for file operations (copy, move, delete, upload, download).
    /// This is the new centralized system that will eventually replace the individual
    /// operation handles (local_copy_operation, batch_download_operation, etc.).
    pub operation_manager: Option<OperationManager>,
    /// Active operation ID for pause/resume from progress modal.
    pub active_operation_id: Option<termide_file_ops::OperationId>,
    /// Last known pause state for active operation (to detect changes).
    pub last_operation_paused: bool,
    /// Timestamp of last mouse scroll event (for throttling heavy operations during scrolling)
    pub last_mouse_scroll: Option<std::time::Instant>,
    /// Flag for batching scroll renders (set on scroll, consumed on tick)
    pub pending_scroll_render: bool,
    /// Flag indicating watcher registration is needed (set on panel add/navigate)
    pub needs_watcher_registration: bool,
    /// Last time user interacted (key/mouse/paste) — for adaptive tick rate
    pub last_activity: std::time::Instant,
    /// Whether the operations panel has stale data that needs a final empty sync
    pub operations_panel_dirty: bool,
    /// Active file operations tracked in Operations panel (keyed by OperationId).
    /// This provides UI state for displaying operation progress in the Operations panel.
    pub active_operations: HashMap<termide_file_ops::OperationId, ActiveOperation>,
    /// Synthetic OperationId for current batch operation (to show in Operations panel).
    /// Maps the real OperationManager operation IDs to this batch entry.
    pub batch_tracking_id: Option<termide_file_ops::OperationId>,
    /// OperationManager ID of the currently running sub-operation within a batch.
    /// Used to bridge pause/cancel from the batch UI to the actual worker.
    pub batch_sub_operation_id: Option<termide_file_ops::OperationId>,
    /// Counter for generating synthetic batch operation IDs.
    batch_id_counter: u64,
    /// Cached shell list for the shell picker submenu (populated on open, cleared on close).
    pub cached_shells: Vec<termide_panel_terminal::shell_utils::ShellInfo>,
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
        let theme = Theme::get_by_name(&config.general.theme);
        Self::with_config_and_theme(config, theme)
    }

    /// Create new application state with given config and theme
    pub fn with_config_and_theme(config: Config, theme: &'static Theme) -> Self {
        let layout_info = LayoutInfo {
            mode: LayoutMode::Single,
            main_panels_count: 1,
        };

        // Create LSP manager if enabled
        let lsp_manager = if config.lsp.enabled {
            let lsp_config = Self::create_lsp_config(&config);
            Some(LspManager::new(lsp_config))
        } else {
            None
        };

        // Load bookmarks from data directory
        let bookmarks = BookmarksConfig::load();

        Self {
            should_quit: false,
            ui: UiState::default(),
            terminal: TerminalState::default(),
            layout_mode: LayoutMode::Single,
            layout_info,
            active_modal: None,
            pending_action: None,
            dir_size_receiver: None,
            git_operation_handle: None,
            script_operation_handle: None,
            pending_editor_download: None,
            pending_batch_upload: None,
            pending_remote_delete: None,
            close_editor_after_upload: None,
            skip_refresh_after_upload: false,
            watcher: None,
            theme,
            config,
            system_monitor: SystemMonitor::new(),
            last_resource_update: std::time::Instant::now(),
            resource_modal_kind: None,
            last_resource_modal_refresh: None,
            last_session_save: None,
            needs_redraw: true, // Initial draw needed
            last_spinner_update: None,
            last_lsp_loading_redraw: None,
            last_git_spinner_update: None,
            lsp_manager,
            all_diagnostics: HashMap::new(),
            bookmarks,
            operation_manager: None, // Will be initialized when VfsManager is available
            active_operation_id: None,
            last_operation_paused: false,
            last_mouse_scroll: None,
            pending_scroll_render: false,
            needs_watcher_registration: true, // Register watchers on first tick
            last_activity: std::time::Instant::now(),
            operations_panel_dirty: false,
            active_operations: HashMap::new(),
            batch_tracking_id: None,
            batch_sub_operation_id: None,
            batch_id_counter: u64::MAX / 2,
            cached_shells: Vec::new(),
        }
    }

    /// Create LSP configuration from app config
    fn create_lsp_config(config: &Config) -> LspConfig {
        let mut servers = std::collections::HashMap::new();

        for (lang, server_config) in &config.lsp.servers {
            servers.insert(
                lang.clone(),
                LspServerConfig {
                    command: server_config.command.clone(),
                    args: server_config.args.clone(),
                    root_markers: server_config.root_markers.clone(),
                },
            );
        }

        LspConfig { servers }
    }

    /// Set new theme and update config
    pub fn set_theme(&mut self, theme_name: &str) {
        self.theme = Theme::get_by_name(theme_name);
        self.config.general.theme = theme_name.to_string();
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
        self.ui.close_all_submenus();
    }

    /// Open submenu (e.g., Preferences dropdown)
    pub fn open_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.options_submenu.open();
    }

    /// Close submenu and all nested menus
    pub fn close_submenu(&mut self) {
        self.ui.options_submenu.close();
        self.ui.nested_submenu.close();
    }

    /// Open Sessions submenu
    pub fn open_sessions_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.sessions_submenu.open();
    }

    /// Close Sessions submenu
    pub fn close_sessions_submenu(&mut self) {
        self.ui.sessions_submenu.close();
    }

    /// Open Tools submenu
    pub fn open_tools_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.tools_submenu.open();
    }

    /// Close Tools submenu
    pub fn close_tools_submenu(&mut self) {
        self.ui.tools_submenu.close();
        self.ui.tools_nested.close();
        self.cached_shells.clear();
    }

    /// Open Tools nested submenu (shell picker) and cache the shell list
    pub fn open_tools_nested_submenu(&mut self, initial_item: usize) {
        self.cached_shells = termide_panel_terminal::shell_utils::discover_shells();
        self.ui.tools_nested.open_at(initial_item);
    }

    /// Close Tools nested submenu and clear cached shells
    pub fn close_tools_nested_submenu(&mut self) {
        self.ui.tools_nested.close();
        self.cached_shells.clear();
    }

    /// Open Scripts submenu
    pub fn open_scripts_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.scripts_submenu.open();
    }

    /// Close Scripts submenu
    pub fn close_scripts_submenu(&mut self) {
        self.ui.scripts_submenu.close();
        self.ui.scripts_nested.close();
        self.ui.current_scripts_group = None;
    }

    /// Open Scripts nested submenu (for a group)
    pub fn open_scripts_nested_submenu(&mut self, group_name: String) {
        self.ui.scripts_nested.open();
        self.ui.current_scripts_group = Some(group_name);
    }

    /// Close Scripts nested submenu
    pub fn close_scripts_nested_submenu(&mut self) {
        self.ui.scripts_nested.close();
        self.ui.current_scripts_group = None;
    }

    /// Open nested submenu (e.g., Themes list)
    pub fn open_nested_submenu(&mut self, initial_item: usize) {
        self.ui.nested_submenu.open_at(initial_item);
    }

    /// Close nested submenu (return to parent submenu)
    pub fn close_nested_submenu(&mut self) {
        self.ui.nested_submenu.close();
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
        self.layout_info = LayoutInfo::calculate(width);
        self.layout_mode = self.layout_info.mode;
    }

    /// Get recommended layout based on terminal width
    pub fn get_recommended_layout(&self) -> &'static str {
        self.layout_info.recommended_layout_str()
    }

    /// Close modal window
    pub fn close_modal(&mut self) {
        self.active_modal = None;
        self.resource_modal_kind = None;
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

    /// Emit terminal bell if enabled in config
    pub fn bell(&self) {
        if self.config.general.bell_on_operation_complete {
            print!("\x07");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }

    /// Create EditorConfig with settings from global config
    pub fn editor_config(&self) -> EditorConfig {
        let mut config = EditorConfig::default();
        config.tab_size = self.config.editor.tab_size;
        config.word_wrap = self.config.editor.word_wrap;
        config.vim_mode = self.config.general.vim_mode;
        config.auto_indent = self.config.editor.auto_indent;
        config.auto_close_brackets = self.config.editor.auto_close_brackets;
        config.keybindings = self.config.editor.keybindings.clone();
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

    /// Save bookmarks to data directory
    pub fn save_bookmarks(&self) {
        if let Err(e) = self.bookmarks.save() {
            log::error!("Failed to save bookmarks: {}", e);
        }
    }

    /// Open bookmarks submenu
    pub fn open_bookmarks_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.bookmarks_submenu.open();
    }

    /// Close bookmarks submenu
    pub fn close_bookmarks_submenu(&mut self) {
        self.ui.bookmarks_submenu.close();
        self.ui.bookmarks_nested.close();
        self.ui.current_bookmarks_group = None;
    }

    /// Open bookmarks nested submenu (for a group)
    pub fn open_bookmarks_nested_submenu(&mut self, group_name: String) {
        self.ui.bookmarks_nested.open();
        self.ui.current_bookmarks_group = Some(group_name);
    }

    /// Close bookmarks nested submenu
    pub fn close_bookmarks_nested_submenu(&mut self) {
        self.ui.bookmarks_nested.close();
        self.ui.current_bookmarks_group = None;
    }

    // ========================================================================
    // Operation Manager Methods
    // ========================================================================

    /// Initialize the operation manager with a VFS manager.
    /// This should be called when the first VFS operation is needed.
    pub fn init_operation_manager(&mut self, vfs_manager: Arc<VfsManager>) {
        if self.operation_manager.is_none() {
            self.operation_manager = Some(OperationManager::new(vfs_manager));
        }
    }

    /// Get reference to operation manager if initialized.
    pub fn operation_manager(&self) -> Option<&OperationManager> {
        self.operation_manager.as_ref()
    }

    /// Get mutable reference to operation manager if initialized.
    pub fn operation_manager_mut(&mut self) -> Option<&mut OperationManager> {
        self.operation_manager.as_mut()
    }

    /// Queue a file operation. Returns the operation ID if successful.
    /// Initializes the operation manager with the provided VFS manager if needed.
    pub fn queue_operation(
        &mut self,
        request: OperationRequest,
        vfs_manager: Arc<VfsManager>,
    ) -> Result<termide_file_ops::OperationId, termide_file_ops::OperationError> {
        self.init_operation_manager(vfs_manager.clone());
        let mgr = self
            .operation_manager_mut()
            .expect("operation_manager just initialized");
        mgr.set_vfs_manager(vfs_manager);
        mgr.queue_operation(request)
    }

    /// Start a file operation immediately (bypassing the queue).
    /// Initializes the operation manager with the provided VFS manager if needed.
    pub fn start_operation_now(
        &mut self,
        request: OperationRequest,
        vfs_manager: Arc<VfsManager>,
    ) -> Result<termide_file_ops::OperationId, termide_file_ops::OperationError> {
        self.init_operation_manager(vfs_manager.clone());
        let mgr = self
            .operation_manager_mut()
            .expect("operation_manager just initialized");
        mgr.set_vfs_manager(vfs_manager);
        mgr.start_now(request)
    }

    /// Poll operation manager for events. Returns empty vec if not initialized.
    pub fn poll_operations(&mut self) -> Vec<OperationEvent> {
        self.operation_manager_mut()
            .map(|m| m.poll())
            .unwrap_or_default()
    }

    /// Check if there are any active or queued operations.
    pub fn has_pending_operations(&self) -> bool {
        self.operation_manager()
            .map(|m| m.has_operations())
            .unwrap_or(false)
    }

    /// Cancel all operations.
    pub fn cancel_all_operations(&mut self) {
        if let Some(manager) = self.operation_manager_mut() {
            manager.cancel_all();
        }
    }

    /// Pause the active operation.
    pub fn pause_active_operation(&mut self) {
        if let Some(id) = self.active_operation_id {
            log::debug!("Pausing operation {}", id);
            if let Some(manager) = self.operation_manager_mut() {
                manager.pause(id);
            }
        } else {
            log::debug!("No active operation to pause");
        }
    }

    /// Resume the active operation.
    pub fn resume_active_operation(&mut self) {
        if let Some(id) = self.active_operation_id {
            log::debug!("Resuming operation {}", id);
            if let Some(manager) = self.operation_manager_mut() {
                manager.resume(id);
            }
        } else {
            log::debug!("No active operation to resume");
        }
    }

    /// Get summary of background operations for status bar display.
    pub fn background_operations_summary(&self) -> Option<BackgroundOperationSummary> {
        self.operation_manager()
            .map(|m| m.background_summary())
            .filter(|s| s.has_operations())
    }

    /// Resolve a conflict for an operation waiting for user decision.
    pub fn resolve_operation_conflict(
        &mut self,
        operation_id: termide_file_ops::OperationId,
        resolution: termide_file_ops::ConflictResolution,
    ) -> bool {
        self.operation_manager_mut()
            .map(|m| m.resolve_conflict(operation_id, resolution))
            .unwrap_or(false)
    }

    // ========================================================================
    // Active Operations Panel Methods
    // ========================================================================

    /// Start tracking a new operation in the Operations panel.
    pub fn track_operation(
        &mut self,
        id: termide_file_ops::OperationId,
        op_type: OperationType,
        source: String,
        dest: String,
        total_files: usize,
        total_bytes: u64,
    ) {
        let op = ActiveOperation::new(id, op_type, source, dest, total_files, total_bytes);
        self.active_operations.insert(id, op);
        self.operations_panel_dirty = true;
    }

    /// Get operations list sorted by start time (newest first).
    pub fn operations_list(&self) -> Vec<&ActiveOperation> {
        let mut ops: Vec<_> = self.active_operations.values().collect();
        ops.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        ops
    }

    /// Find index of operation by ID in the sorted list.
    pub fn operation_index(&self, id: termide_file_ops::OperationId) -> Option<usize> {
        self.operations_list().iter().position(|op| op.id == id)
    }

    /// Check if there are any active operations being tracked.
    pub fn has_active_operations(&self) -> bool {
        !self.active_operations.is_empty()
    }

    /// Remove an operation from tracking (e.g., when completed/cancelled).
    pub fn untrack_operation(&mut self, id: termide_file_ops::OperationId) {
        if self.active_operations.remove(&id).is_some() {
            self.operations_panel_dirty = true;
        }
    }

    /// Start tracking a batch operation.
    /// Returns synthetic OperationId for the batch.
    pub fn start_batch_tracking(
        &mut self,
        op_type: OperationType,
        source: String,
        dest: String,
        total_files: usize,
        total_bytes: u64,
    ) -> termide_file_ops::OperationId {
        // Generate synthetic ID (wraps around if exhausted, which is practically impossible)
        self.batch_id_counter = self.batch_id_counter.wrapping_add(1);
        let batch_id = termide_file_ops::OperationId::new(self.batch_id_counter);

        // Create tracked operation
        self.track_operation(batch_id, op_type, source, dest, total_files, total_bytes);
        self.batch_tracking_id = Some(batch_id);

        batch_id
    }

    /// Finish batch tracking - remove the batch operation from active_operations.
    pub fn finish_batch_tracking(&mut self) {
        if let Some(batch_id) = self.batch_tracking_id.take() {
            if self.active_operations.remove(&batch_id).is_some() {
                self.operations_panel_dirty = true;
            }
        }
        self.batch_sub_operation_id = None;
    }

    /// Update batch tracked operation file-level progress.
    /// Only updates file counts; byte-level progress is managed by poll_operation_manager.
    pub fn update_batch_progress(&mut self, files_completed: usize, total_files: usize) {
        if let Some(batch_id) = self.batch_tracking_id {
            if let Some(op) = self.active_operations.get_mut(&batch_id) {
                op.progress.files_completed = files_completed;
                op.progress.total_files = total_files;
            }
        }
    }

    /// Set batch tracked operation pause state.
    pub fn set_batch_paused(&mut self, paused: bool) {
        if let Some(batch_id) = self.batch_tracking_id {
            if let Some(op) = self.active_operations.get_mut(&batch_id) {
                op.is_paused = paused;
                if paused {
                    op.speed_tracker.reset();
                }
            }
        }
    }
}

// ============================================================================
// Core Trait Implementations
// ============================================================================

impl StateManager for AppState {
    fn ui(&self) -> &UiState {
        &self.ui
    }

    fn ui_mut(&mut self) -> &mut UiState {
        &mut self.ui
    }

    fn set_info(&mut self, msg: String) {
        self.ui.status_message = Some((msg, false));
    }

    fn set_error(&mut self, msg: String) {
        self.ui.status_message = Some((msg, true));
    }

    fn clear_status(&mut self) {
        self.ui.status_message = None;
    }

    fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    fn set_redraw(&mut self, value: bool) {
        self.needs_redraw = value;
    }
}

impl ModalManager for AppState {
    fn active_modal(&self) -> Option<&ActiveModal> {
        self.active_modal.as_ref()
    }

    fn active_modal_mut(&mut self) -> Option<&mut ActiveModal> {
        self.active_modal.as_mut()
    }

    fn open_modal(&mut self, modal: ActiveModal, action: Option<PendingAction>) {
        self.active_modal = Some(modal);
        self.pending_action = action;
    }

    fn close_modal(&mut self) {
        self.active_modal = None;
        self.resource_modal_kind = None;
    }

    fn take_pending_action(&mut self) -> Option<PendingAction> {
        self.pending_action.take()
    }
}
