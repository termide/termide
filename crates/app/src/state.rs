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
use termide_file_ops::OperationManager;
use termide_lsp::{LspConfig, LspManager, LspServerConfig};
use termide_panel_editor::EditorConfig;
use termide_system_monitor::SystemMonitor;
use termide_theme::Theme;
use termide_watcher::UnifiedWatcher;

// Import core traits
use termide_app_core::{ModalManager, StateManager};

// Re-export pure types from state crate
pub use termide_state::{
    ActiveOperation, BatchOperation, BatchOperationType, ConflictMode, DirSizeResult, LayoutInfo,
    LayoutMode, OperationProgress, OperationType, PendingAction, ProjectsOrigin, RenamePattern,
    SourceLocation, SpeedTracker, SubmenuState, TerminalState, UiState,
};

// Re-export ActiveModal from modal crate
pub use termide_modal::ActiveModal;

// Re-export auxiliary state types (moved to `state_types`) so their public
// paths under `crate::state` remain unchanged.
pub use crate::state_types::{
    kill_process_tree, BatchOperationState, CacheState, CommandOperationHandle,
    CommandOperationResult, GitOperationHandle, GitOperationResult, PendingBatchUpload,
    PendingEditorDownload, PendingRemoteDelete, ResourceModalKind, ScrollBarDrag, StashState,
};

/// A viewer URL fetch in flight: the id its viewer waits under and the channel
/// the worker thread answers on, with the document or a readable error.
#[derive(Debug)]
pub struct ViewFetch {
    pub id: u64,
    pub receiver: mpsc::Receiver<Result<termide_fetch::Fetched, String>>,
}

/// Global application state
#[derive(Debug)]
pub struct AppState {
    /// Should application quit
    pub should_quit: bool,
    /// UI components state
    pub ui: UiState,
    /// Scrollbar thumb being dragged with the mouse, if any.
    pub scrollbar_drag: Option<ScrollBarDrag>,
    /// Terminal state
    pub terminal: TerminalState,
    /// Current layout mode
    pub layout_mode: LayoutMode,
    /// Current layout information
    pub layout_info: LayoutInfo,
    /// Active modal window
    pub active_modal: Option<ActiveModal>,
    /// Reports that finished while another modal was open, oldest first. A
    /// report's text exists only in its modal, so it is never dropped: it
    /// waits here until the screen is clear. Bounded — the oldest goes when
    /// full, since a stale build report is worth less than a live question.
    pub queued_info_modals: Vec<ActiveModal>,
    /// Action pending modal result
    pub pending_action: Option<PendingAction>,
    /// Receiver channel for background directory size calculation results
    pub dir_size_receiver: Option<mpsc::Receiver<DirSizeResult>>,
    /// Viewer URL fetches in flight (`Ctrl+G` with an `http(s)://` address, a
    /// followed link, a history step), each delivered to the viewer that
    /// waits under its id.
    pub view_fetches: Vec<ViewFetch>,
    /// Id for the next viewer URL fetch.
    pub next_view_fetch_id: u64,
    /// Handle for background git operation (allows cancellation)
    pub git_operation_handle: Option<GitOperationHandle>,
    /// SSH key passphrase entered for git network operations, cached in memory
    /// for the session so repeated push/fetch don't re-prompt. Never persisted.
    pub git_ssh_passphrase: Option<String>,
    /// Handles for background command operations (.report. commands)
    pub command_operation_handles: Vec<CommandOperationHandle>,
    /// Handles for background commands (.bg.) tracked in Operations panel: (op_id, receiver, pid)
    pub bg_command_handles: Vec<(termide_file_ops::OperationId, mpsc::Receiver<()>, u32)>,
    /// Pending editor download via OperationManager (replaces download_operation for editor opens)
    pub pending_editor_download: Option<PendingEditorDownload>,
    /// Grouped state for an in-flight batch operation (upload/delete/tracking).
    pub batch: BatchOperationState,
    /// Close editor after current upload completes (for "save and close" flow).
    /// Stores the file path of the editor to close (to find the correct panel).
    pub close_editor_after_upload: Option<PathBuf>,
    /// Skip file manager refresh after upload (for editor saves - file already exists)
    pub skip_refresh_after_upload: bool,
    /// Unified watcher for filesystem and git changes
    pub watcher: Option<UnifiedWatcher>,
    /// Current theme
    pub theme: &'static Theme,
    /// Application configuration (effective: built-in defaults + global file
    /// + per-project override). Mutated as the user changes settings.
    pub config: Arc<Config>,
    /// Snapshot of `defaults + global file` taken at startup, *without* the
    /// per-project overlay. Used as the diff baseline when saving the
    /// per-project override file so only project-specific deltas are
    /// recorded.
    pub global_baseline: Arc<Config>,
    /// System resource monitor (CPU, RAM)
    pub system_monitor: SystemMonitor,
    /// Last time system resources were updated
    pub last_resource_update: std::time::Instant,
    /// Currently open resource modal (for auto-refresh in tick)
    pub resource_modal_kind: Option<ResourceModalKind>,
    /// Last time resource modal was refreshed
    pub last_resource_modal_refresh: Option<std::time::Instant>,
    /// Last time the layout was saved (for debouncing autosave)
    pub last_layout_save: Option<std::time::Instant>,
    /// Flag indicating UI needs to be redrawn (for CPU optimization)
    pub needs_redraw: bool,

    /// Whether this termide is hosted in a detachable instance.
    ///
    /// Decided once at startup — a process cannot become detachable later —
    /// and read by the menu, so that rendering, clicking and key handling all
    /// agree on whether the Detach entry exists.
    pub detach_available: bool,
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
    /// Project-local bookmarks from `.termide/bookmarks.toml` (read-only overlay)
    pub project_bookmarks: Option<BookmarksConfig>,
    /// Project root path (for loading project-local .termide/ configs)
    pub project_root: PathBuf,
    /// Projects open in this instance, for the menus to show (they sort it).
    /// `App` keeps it in step with the parked panels it holds.
    pub open_projects: Vec<crate::open_projects::OpenProjectView>,
    /// The open project whose menu-bar button the mouse holds, by its
    /// place: it keeps its button on a crowded bar while dragged.
    pub held_project_button: Option<usize>,
    /// Projects open together in the last run, offered by the Projects menu
    /// until they are reopened or another project is opened by hand.
    pub reopenable_projects: Vec<PathBuf>,
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
    /// Last time operations panel was redrawn for elapsed time update (throttled to 1s)
    pub last_operations_elapsed_redraw: Option<std::time::Instant>,
    /// Active file operations tracked in Operations panel (keyed by OperationId).
    /// This provides UI state for displaying operation progress in the Operations panel.
    pub active_operations: HashMap<termide_file_ops::OperationId, ActiveOperation>,
    // Batch-operation fields moved to `batch: BatchOperationState` above.
    /// Cached shell list for the shell picker submenu (populated on open, cleared on close).
    pub stash: StashState,
    /// Cached menus, commands registry, disk space.
    pub cache: CacheState,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// A localized "N units ago" for a millisecond timestamp, for the Sessions
/// list's "last worked on" column. Reuses [`termide_i18n::relative_age`].
fn relative_millis_ago(modified_ms: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    termide_i18n::relative_age(now_ms.saturating_sub(modified_ms) / 1000)
}

impl AppState {
    /// Create new application state, loading config from file
    pub fn new() -> Self {
        let config = Config::load().unwrap_or_else(|e| {
            log::warn!("Could not load config: {}. Using defaults.", e);
            Config::default()
        });
        let theme = Theme::get_by_name(&config.general.theme);
        let baseline = config.clone();
        Self::with_config_and_theme(config, baseline, theme)
    }

    /// Create new application state with given config, project-overlay
    /// baseline, and theme. `global_baseline` should be the `Config`
    /// produced by overlaying the global file on `Config::default()` —
    /// what `Config::load_layered` returns as its second tuple element.
    /// When no project file exists this baseline equals `config`.
    pub fn with_config_and_theme(
        config: Config,
        global_baseline: Config,
        theme: &'static Theme,
    ) -> Self {
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
            scrollbar_drag: None,
            terminal: TerminalState::default(),
            layout_mode: LayoutMode::Single,
            layout_info,
            active_modal: None,
            queued_info_modals: Vec::new(),
            pending_action: None,
            dir_size_receiver: None,
            view_fetches: Vec::new(),
            next_view_fetch_id: 0,
            git_operation_handle: None,
            git_ssh_passphrase: None,
            command_operation_handles: Vec::new(),
            bg_command_handles: Vec::new(),
            pending_editor_download: None,
            batch: BatchOperationState {
                id_counter: u64::MAX / 2,
                ..Default::default()
            },
            close_editor_after_upload: None,
            skip_refresh_after_upload: false,
            watcher: None,
            theme,
            config: Arc::new(config),
            global_baseline: Arc::new(global_baseline),
            system_monitor: SystemMonitor::new(),
            last_resource_update: std::time::Instant::now(),
            resource_modal_kind: None,
            last_resource_modal_refresh: None,
            last_layout_save: None,
            needs_redraw: true,
            detach_available: false, // Initial draw needed
            last_spinner_update: None,
            last_lsp_loading_redraw: None,
            last_git_spinner_update: None,
            lsp_manager,
            all_diagnostics: HashMap::new(),
            bookmarks,
            project_bookmarks: None,
            project_root: std::env::current_dir().unwrap_or_default(),
            open_projects: Vec::new(),
            held_project_button: None,
            reopenable_projects: Vec::new(),
            operation_manager: None, // Will be initialized when VfsManager is available
            active_operation_id: None,
            last_operation_paused: false,
            last_mouse_scroll: None,
            pending_scroll_render: false,
            needs_watcher_registration: true, // Register watchers on first tick
            last_activity: std::time::Instant::now(),
            operations_panel_dirty: false,
            last_operations_elapsed_redraw: None,
            active_operations: HashMap::new(),
            stash: StashState::default(),
            cache: CacheState::default(),
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
        // Copy-on-write: clone config, modify, replace Arc
        let mut config = (*self.config).clone();
        config.general.theme = theme_name.to_string();
        self.config = Arc::new(config);
    }

    /// Request application quit
    pub fn quit(&mut self) {
        // Logged: without this a quit and a crash look the same in the
        // journal — the run simply stops, with no record of why.
        log::info!("Quit requested");
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
        self.cache.shells.clear();
        self.cache.commands_registry = None;
        self.cache.projects.clear();
        // Note: hotkey_table is NOT invalidated here — key bindings don't
        // change when a menu closes. Only invalidated on config/command changes.
    }

    /// Close resource indicator modal (CPU/RAM/Net/Calendar) and clear refresh state.
    pub fn close_indicator_modal(&mut self) {
        self.active_modal = None;
        self.resource_modal_kind = None;
        self.last_resource_modal_refresh = None;
    }

    /// Open submenu (e.g., Preferences dropdown)
    pub fn open_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.options_submenu.open();
    }

    /// Open Projects submenu
    pub fn open_projects_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.projects_submenu.open();
        self.load_known_projects();
    }

    /// Open Tools submenu
    pub fn open_tools_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.tools_submenu.open();
    }

    /// Open Commands submenu
    pub fn open_commands_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.commands_submenu.open();
        // Force fresh load — the cache may have been populated before the
        // filesystem made command files visible (FUSE/autofs on NixOS, etc.).
        self.cache.commands_registry = None;
        // hotkey_table not invalidated — bindings don't change on submenu open
    }

    /// Open Tools nested submenu (shell picker) and cache the shell list
    pub fn open_tools_nested_submenu(&mut self, initial_item: usize) {
        self.cache.shells = termide_panel_terminal::shell_utils::discover_shells();
        self.ui.tools_nested.open_at(initial_item);
    }

    /// Close Tools nested submenu and clear cached shells
    pub fn close_tools_nested_submenu(&mut self) {
        self.ui.tools_nested.close();
        self.cache.shells.clear();
    }

    /// Open Commands nested submenu (for a group)
    pub fn open_commands_nested_submenu(&mut self, group_name: String) {
        self.ui.commands_nested.open();
        self.ui.current_commands_group = Some(group_name);
    }

    /// Close Commands nested submenu
    pub fn close_commands_nested_submenu(&mut self) {
        self.ui.commands_nested.close();
        self.ui.current_commands_group = None;
    }

    /// Open the AI submenu (Agents / Sessions / Skills / Prompts).
    pub fn open_ai_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.ai_submenu.open();
    }

    /// Open the AI nested submenu for `section` (the section's item list).
    pub fn open_ai_nested_submenu(&mut self, section: termide_state::AiSection) {
        self.ui.ai_nested.open();
        self.ui.current_ai_section = Some(section);
    }

    /// Close the AI nested submenu.
    pub fn close_ai_nested_submenu(&mut self) {
        self.ui.ai_nested.close();
        self.ui.current_ai_section = None;
    }

    /// The layered AI resource roots for this project (cwd/project/global). The
    /// menu is project-scoped, so the working dir is the project root.
    pub(crate) fn ai_dirs(&self) -> termide_agent_core::AgentDirs {
        let global = termide_config::get_config_dir()
            .ok()
            .map(|d| d.join(termide_agent_core::GLOBAL_AGENT_DIR));
        termide_agent_core::AgentDirs::new(
            &self.project_root,
            Some(&self.project_root),
            global.as_deref(),
        )
    }

    /// The session-log directory for this project (`<config>/ai/sessions/<key>`).
    pub(crate) fn ai_sessions_dir(&self) -> Option<PathBuf> {
        let dir = termide_config::get_config_dir().ok()?;
        Some(
            dir.join(termide_agent_core::GLOBAL_AGENT_DIR)
                .join(termide_agent_core::SESSIONS_DIR)
                .join(termide_project::project_key(&self.project_root)),
        )
    }

    /// The Sessions section's rows: the project's session logs, newest first.
    fn ai_session_items(&self) -> Vec<termide_ui_render::DropdownItem> {
        use termide_ui_render::DropdownItem;
        let sessions = self
            .ai_sessions_dir()
            .and_then(|dir| termide_agent_core::Session::list(&dir).ok())
            .unwrap_or_default();
        if sessions.is_empty() {
            return vec![DropdownItem::new(
                termide_i18n::t().ai_empty(),
                String::new(),
            )];
        }
        sessions
            .into_iter()
            .map(|s| {
                DropdownItem::new(s.label(), format!("session:{}", s.path.to_string_lossy()))
                    .with_shortcut(Some(relative_millis_ago(s.modified)))
            })
            .collect()
    }

    /// Build one AI section's dropdown rows. Called by both the renderer and the
    /// action handler so they address the same rows by index. Agents, skills and prompts get
    /// two "New …" rows and a separator before the merged, source-marked items
    /// (project-local first, in bold); sessions list the project's logs only.
    pub fn ai_section_items(
        &self,
        section: termide_state::AiSection,
    ) -> Vec<termide_ui_render::DropdownItem> {
        use termide_state::AiSection;
        use termide_ui_render::DropdownItem;
        let t = termide_i18n::t();

        let dirs = self.ai_dirs();
        // (name, description, is_project)
        let mut listed: Vec<(String, String, bool)> = match section {
            AiSection::Sessions => return self.ai_session_items(),
            AiSection::Agents => dirs
                .agents()
                .into_iter()
                .map(|name| {
                    let is_project = dirs
                        .agent_dir(&name)
                        .map(|p| p.starts_with(&self.project_root))
                        .unwrap_or(false);
                    let description = dirs.spec(&name).description;
                    (name, description, is_project)
                })
                .collect(),
            AiSection::Skills => dirs
                .skills()
                .into_iter()
                .map(|s| {
                    let is_project = s.path.starts_with(&self.project_root);
                    (s.name, s.description, is_project)
                })
                .collect(),
            AiSection::Prompts => dirs
                .prompts()
                .into_iter()
                .map(|p| {
                    let is_project = dirs
                        .prompt_path(&p.name)
                        .map(|path| path.starts_with(&self.project_root))
                        .unwrap_or(false);
                    (p.name, p.description, is_project)
                })
                .collect(),
        };
        let mut items = vec![
            DropdownItem::new(t.menu_ai_new_project(), "new:project"),
            DropdownItem::new(t.menu_ai_new_global(), "new:global"),
            DropdownItem::separator(),
        ];
        // Project-local first (bold), then global; each group alphabetical.
        listed.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        let has_project = listed.iter().any(|(_, _, p)| *p);
        let has_global = listed.iter().any(|(_, _, p)| !*p);
        let mut pushed_sep = false;
        for (name, description, is_project) in listed {
            if !is_project && has_project && has_global && !pushed_sep {
                items.push(DropdownItem::separator());
                pushed_sep = true;
            }
            // The key keeps the bare name; the label adds the one-line
            // description the same way the panel's pickers show it.
            let description = description.lines().next().unwrap_or("").trim();
            let label = if description.is_empty() {
                name.clone()
            } else {
                format!("{name} · {description}")
            };
            let mut item = DropdownItem::new(label, format!("item:{name}"));
            if is_project {
                item = item.with_project();
            }
            items.push(item);
        }
        items
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

    /// Move the menu bar selection one step along `order` (the positions
    /// as the bar shows them), wrapping around. A selection no longer in
    /// `order` — a project button that went away — restarts at its start.
    pub fn step_menu_item(&mut self, order: &[usize], forward: bool) {
        let Some(current) = self.ui.selected_menu_item else {
            return;
        };
        if order.is_empty() {
            return;
        }
        let next = match order.iter().position(|&index| index == current) {
            Some(at) if forward => (at + 1) % order.len(),
            Some(at) => (at + order.len() - 1) % order.len(),
            None => 0,
        };
        self.ui.selected_menu_item = Some(order[next]);
        self.ui.selected_dropdown_item = 0;
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

    /// Check if the main menu is open.
    #[inline]
    pub fn is_menu_open(&self) -> bool {
        self.ui.menu_open
    }

    /// Check if a resource-indicator modal (Disk/CPU/RAM/Network) is currently open.
    #[inline]
    pub fn is_resource_modal_open(&self) -> bool {
        self.resource_modal_kind.is_some()
            && matches!(self.active_modal, Some(ActiveModal::Info(_)))
    }

    /// Get immutable reference to the active modal.
    #[inline]
    pub fn active_modal(&self) -> Option<&ActiveModal> {
        self.active_modal.as_ref()
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

    /// Show a message that interrupts nothing: a report, an error, a result.
    ///
    /// A question already on screen — one that carries a `pending_action` —
    /// keeps the screen: its answer decides a real operation, and replacing it
    /// would lose both the question and whatever was typed into it. The message
    /// waits instead. With no question open, or over a plain message, it shows
    /// now; over a message it replaces it, the older one having been read.
    pub fn show_message_modal(&mut self, modal: ActiveModal) {
        let asking = self.active_modal.is_some() && self.pending_action.is_some();
        if asking {
            self.queued_info_modals.push(modal);
            // A report nobody will ever reach is worth less than the live
            // question ahead of it, and an unbounded queue never drains.
            const MAX_QUEUED: usize = 8;
            while self.queued_info_modals.len() > MAX_QUEUED {
                self.queued_info_modals.remove(0);
            }
            log::debug!(
                "A modal is open; queued a message behind it ({} waiting)",
                self.queued_info_modals.len()
            );
        } else {
            // The message that was here — a resource panel, an earlier report —
            // is gone, and with it the kind that made it a resource panel.
            // Leaving the kind set would have Esc and the arrows treat this
            // report as that panel.
            self.resource_modal_kind = None;
            self.active_modal = Some(modal);
        }
    }

    /// The next message waiting for the screen, if any.
    fn take_queued_message_modal(&mut self) -> Option<ActiveModal> {
        if self.active_modal.is_none() && !self.queued_info_modals.is_empty() {
            Some(self.queued_info_modals.remove(0))
        } else {
            None
        }
    }

    /// Give the screen to a message that was queued behind a question, if the
    /// screen is clear now. Returns whether it did.
    pub fn deliver_queued_message_modal(&mut self) -> bool {
        match self.take_queued_message_modal() {
            Some(modal) => {
                self.active_modal = Some(modal);
                true
            }
            None => false,
        }
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

    /// Ring the terminal bell for an agent panel that waits for the user,
    /// if enabled in config
    pub fn attention_bell(&self) {
        if self.config.ai.bell_on_attention {
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

    /// Check if enough time has passed since the last layout save (debounce check)
    /// Returns true if we should save the layout
    pub fn should_save_layout(&self) -> bool {
        const DEBOUNCE_DURATION: std::time::Duration = std::time::Duration::from_secs(1);

        match self.last_layout_save {
            None => true, // Never saved before
            Some(last_save) => last_save.elapsed() >= DEBOUNCE_DURATION,
        }
    }

    /// Update last layout save timestamp
    pub fn update_last_layout_save(&mut self) {
        self.last_layout_save = Some(std::time::Instant::now());
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

    /// Open bookmarks nested submenu (for a group)
    pub fn open_bookmarks_nested_submenu(&mut self, group_name: String, is_project: bool) {
        self.ui.bookmarks_nested.open();
        self.ui.current_bookmarks_group = Some(group_name);
        self.ui.current_bookmarks_group_is_project = is_project;
    }

    /// Close bookmarks nested submenu
    pub fn close_bookmarks_nested_submenu(&mut self) {
        self.ui.bookmarks_nested.close();
        self.ui.current_bookmarks_group = None;
        self.ui.current_bookmarks_group_is_project = false;
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

#[cfg(test)]
mod message_queue_tests {
    use super::{ActiveModal, AppState, PendingAction};
    use termide_modal::{ConfirmModal, InfoModal};

    /// State on the built-in defaults: `AppState::new` would read the
    /// developer's own config file.
    fn state() -> AppState {
        let mut config = termide_config::Config::default();
        config.normalize();
        let theme = termide_theme::Theme::get_by_name(&config.general.theme);
        AppState::with_config_and_theme(config.clone(), config, theme)
    }

    fn report(title: &str) -> ActiveModal {
        ActiveModal::Info(Box::new(InfoModal::new(
            title,
            vec![(String::new(), "output".to_string())],
        )))
    }

    fn question(title: &str) -> ActiveModal {
        ActiveModal::Confirm(Box::new(ConfirmModal::new(title, "are you sure?")))
    }

    /// A report's text exists nowhere else, so a question on screen must not
    /// cost it: the report waits and takes the screen once the question is
    /// answered. Dropping it would abandon a copy mid-flight with no record.
    #[test]
    fn a_report_waits_behind_an_open_question() {
        let mut s = state();
        s.set_pending_action(PendingAction::QuitApplication, question("Quit?"));

        s.show_message_modal(report("build"));

        assert!(
            matches!(s.active_modal(), Some(ActiveModal::Confirm(_))),
            "the question keeps the screen"
        );
        assert!(!s.deliver_queued_message_modal(), "nothing to take yet");

        s.close_modal();
        assert!(s.deliver_queued_message_modal());
        assert!(format!("{:?}", s.active_modal()).contains("build"));
        assert!(!s.deliver_queued_message_modal(), "the queue drained once");
    }

    /// Over a plain message — no question behind it — the newer report wins
    /// the screen at once; the older one has been read.
    #[test]
    fn a_report_over_a_plain_message_shows_at_once() {
        let mut s = state();
        s.show_message_modal(report("first"));
        s.show_message_modal(report("second"));

        assert!(s.queued_info_modals.is_empty());
        assert!(format!("{:?}", s.active_modal()).contains("second"));
    }

    /// The queue is bounded: a session that never clears the screen must not
    /// grow it without limit, and the stalest report is the least worth kept.
    #[test]
    fn the_queue_keeps_the_newest_and_drops_the_stalest() {
        let mut s = state();
        s.set_pending_action(PendingAction::QuitApplication, question("Quit?"));
        for i in 0..12 {
            s.show_message_modal(report(&format!("report-{i}")));
        }

        assert_eq!(s.queued_info_modals.len(), 8);
        s.close_modal();
        let first = s.take_queued_message_modal();
        assert!(format!("{first:?}").contains("report-4"), "the oldest kept");
    }

    /// A resource indicator is an `Info` modal whose Esc and arrows come from
    /// `resource_modal_kind`. A report replacing it must take that flag with
    /// it, or Esc would close the wrong thing.
    #[test]
    fn a_report_replacing_a_resource_panel_clears_its_kind() {
        use crate::state::ResourceModalKind;

        let mut s = state();
        s.show_message_modal(report("disk"));
        s.resource_modal_kind = Some(ResourceModalKind::Disk);

        s.show_message_modal(report("build"));

        assert!(s.resource_modal_kind.is_none());
    }
}
