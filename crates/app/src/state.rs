//! Application state and types.
//!
//! Re-exports pure types from termide-state crate and defines
//! complex types that depend on other application modules.
//!
//! Implements core traits from termide-app-core for standardized
//! state management and modal handling.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use termide_config::{BookmarksConfig, Config};
use termide_file_ops::OperationManager;
use termide_lsp::{LspConfig, LspManager, LspServerConfig};
use termide_panel_editor::EditorConfig;
use termide_system_monitor::SystemMonitor;
use termide_theme::Theme;
use termide_watcher::{UnifiedWatcher, WatchTarget};

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
    CommandOperationResult, EditorUpload, GitOperationHandle, GitOperationResult,
    PendingBatchUpload, PendingCommandLaunch, PendingEditorDownload, PendingRemoteDelete,
    ResourceModalKind, ScrollBarDrag, SizeScan, StashState,
};

/// The key prefix of a Sessions row in the AI menu, before the directory
/// its sessions are filed in.
pub(crate) const SESSION_LOCATION_KEY: &str = "location:";

/// Where agent session logs are filed, `<config>/ai/sessions`.
pub(crate) fn ai_sessions_root() -> Option<PathBuf> {
    termide_config::get_config_dir().ok().map(|dir| {
        dir.join(termide_agent_core::GLOBAL_AGENT_DIR)
            .join(termide_agent_core::SESSIONS_DIR)
    })
}

/// What a git job started from the interface does, which decides how its end
/// is reported.
#[derive(Debug, Clone, PartialEq)]
pub enum GitJobKind {
    /// A commit from the commit window; answers with the new short hash.
    Commit,
    StashPush,
    StashPop(usize),
    StashApply(usize),
    StashDrop(usize),
    StashRename,
    Stage,
    Unstage,
    /// One file's changes reverted.
    Revert,
    /// Every change unstaged and reverted.
    RevertAll,
    /// The stash list for the stash dropdown, which opens when it answers
    /// if the project it was asked in is still the one shown.
    StashList {
        button_area: ratatui::layout::Rect,
        has_changes: bool,
        project_root: PathBuf,
    },
    /// The details of stash `index` for its window, which opens when they
    /// come if the stash dropdown is still open on the project it was asked
    /// in.
    StashInfo {
        index: usize,
        project_root: PathBuf,
    },
}

/// What a git job answers with when it succeeds.
#[derive(Debug)]
pub enum GitJobOutput {
    Done,
    /// The new commit's short hash.
    Committed(String),
    Stashes(Vec<termide_git::StashEntry>),
    /// A stash's details; `None` when git could not tell them.
    StashDetails(Option<termide_git::StashInfo>),
}

/// A git command started from the interface, in flight: a commit's hooks may
/// run for minutes and any command can wait on a slow disk or a large
/// repository, so it runs on a worker thread that answers with its output or
/// the message to show for its failure, already worded.
#[derive(Debug)]
pub struct GitJob {
    pub repo_path: PathBuf,
    pub kind: GitJobKind,
    pub receiver: mpsc::Receiver<Result<GitJobOutput, String>>,
}

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
    /// Questions raised in the background (a host key, an archive password)
    /// while another question was open, oldest first. Each has a panel
    /// waiting for its answer, so none is ever dropped: they take the screen
    /// one by one as it clears, ahead of the queued reports.
    pub queued_questions: std::collections::VecDeque<(PendingAction, ActiveModal)>,
    /// Action pending modal result
    pub pending_action: Option<PendingAction>,
    /// Receiver channel for background directory size calculation results
    pub dir_size_receiver: Option<mpsc::Receiver<DirSizeResult>>,
    /// The agents' usage window's counting of the logs, and what it keeps.
    pub(crate) usage: crate::app::usage::UsageState,
    /// Viewer URL fetches in flight (`Ctrl+G` with an `http(s)://` address, a
    /// followed link, a history step), each delivered to the viewer that
    /// waits under its id.
    pub view_fetches: Vec<ViewFetch>,
    /// Id for the next viewer URL fetch.
    pub next_view_fetch_id: u64,
    /// Handle for background git operation (allows cancellation)
    pub git_operation_handle: Option<GitOperationHandle>,
    /// Git commands started from the interface still running: commits,
    /// stash, stage and revert actions, the stash dropdown's list.
    pub git_jobs: Vec<GitJob>,
    /// Sources of finished moves from a remote, still being deleted there:
    /// a source that stays is reported, since the move then only copied.
    pub remote_move_deletes: Vec<(termide_vfs::VfsPath, termide_vfs::VfsOperation<()>)>,
    /// SSH key passphrase entered for git network operations, cached in memory
    /// for the session so repeated push/fetch don't re-prompt. Never persisted.
    pub git_ssh_passphrase: Option<String>,
    /// Handles for background command operations (.report. commands)
    pub command_operation_handles: Vec<CommandOperationHandle>,
    /// Handles for background commands (.bg.) tracked in Operations panel: (op_id, receiver, pid)
    pub bg_command_handles: Vec<(termide_file_ops::OperationId, mpsc::Receiver<()>, u32)>,
    /// Commands waiting for their directory's first direnv environment
    pub pending_command_launches: Vec<PendingCommandLaunch>,
    /// Pending editor download via OperationManager (replaces download_operation for editor opens)
    pub pending_editor_download: Option<PendingEditorDownload>,
    /// Grouped state for an in-flight batch operation (upload/delete/tracking).
    pub batch: BatchOperationState,
    /// Remote editors' saves being uploaded, each settled when its own
    /// operation ends
    pub editor_uploads: Vec<EditorUpload>,
    /// Unified watcher for filesystem and git changes
    pub watcher: Option<UnifiedWatcher>,
    /// Watches the app holds on `watcher`: one per panel wanting the target.
    /// Reconciled against the panels by `register_panel_watchers`.
    pub held_watches: HashMap<WatchTarget, usize>,
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
    /// What this run forces on `config` (`--log-level`, `--no-lsp`, the
    /// Linux console's theme). `config` holds the forced values; they are
    /// swapped back before a save, so the files never get them.
    /// `global_baseline` holds the values from the files.
    pub runtime_overrides: termide_config::RuntimeOverrides,
    /// System resource monitor (CPU, RAM)
    pub system_monitor: SystemMonitor,
    /// Last time system resources were updated
    pub last_resource_update: std::time::Instant,
    /// The menu bar clock's time at the last resource update: a new minute
    /// is redrawn on the next update, as nothing else redraws the clock.
    pub clock_shown: String,
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
    /// The root of the project open now, whose approvals say which
    /// workspaces a language server that runs their code may start in;
    /// shared with the manager, which asks at each start.
    pub lsp_trust_root: std::sync::Arc<std::sync::Mutex<PathBuf>>,
    /// Workspaces whose language server was held back, still to be asked
    /// about, one window at a time.
    pub lsp_trust_asks: std::collections::VecDeque<termide_lsp::WithheldServer>,
    /// All diagnostics from LSP servers, keyed by file path
    pub all_diagnostics: HashMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// What the language servers last published for each file; with
    /// `plugin_diagnostics`, what `all_diagnostics` is made of.
    pub lsp_diagnostics: HashMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// What each plugin set for each file, by the plugin's project and name.
    pub plugin_diagnostics:
        HashMap<PathBuf, std::collections::BTreeMap<(PathBuf, String), Vec<lsp_types::Diagnostic>>>,
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

/// The approvals of the project rooted at `project_root`: its launches and
/// trusted workspaces, kept in termide's data directory.
pub(crate) fn workspace_approvals(project_root: &Path) -> termide_agent_core::approvals::Approvals {
    termide_agent_core::approvals::Approvals::new(
        termide_project::approvals_path(project_root).ok(),
    )
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
        let project_root = std::env::current_dir().unwrap_or_default();
        let lsp_trust_root = std::sync::Arc::new(std::sync::Mutex::new(project_root.clone()));
        let lsp_manager = config
            .lsp
            .enabled
            .then(|| Self::new_lsp_manager(Self::create_lsp_config(&config), &lsp_trust_root));

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
            queued_questions: std::collections::VecDeque::new(),
            pending_action: None,
            dir_size_receiver: None,
            usage: crate::app::usage::UsageState::default(),
            view_fetches: Vec::new(),
            next_view_fetch_id: 0,
            git_operation_handle: None,
            git_jobs: Vec::new(),
            remote_move_deletes: Vec::new(),
            git_ssh_passphrase: None,
            command_operation_handles: Vec::new(),
            bg_command_handles: Vec::new(),
            pending_command_launches: Vec::new(),
            pending_editor_download: None,
            batch: BatchOperationState {
                id_counter: u64::MAX / 2,
                ..Default::default()
            },
            editor_uploads: Vec::new(),
            watcher: None,
            held_watches: HashMap::new(),
            theme,
            config: Arc::new(config),
            global_baseline: Arc::new(global_baseline),
            runtime_overrides: termide_config::RuntimeOverrides::default(),
            system_monitor: SystemMonitor::new(),
            last_resource_update: std::time::Instant::now(),
            clock_shown: termide_ui_render::menu::clock_text(),
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
            lsp_diagnostics: HashMap::new(),
            plugin_diagnostics: HashMap::new(),
            bookmarks,
            project_bookmarks: None,
            project_root,
            lsp_trust_root,
            lsp_trust_asks: std::collections::VecDeque::new(),
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

    /// A language server manager for `config`. A server that runs the code
    /// of the workspace it analyses ([`termide_lsp::runs_workspace_code`])
    /// starts only in a workspace the project open at the time trusts.
    pub(crate) fn new_lsp_manager(
        config: LspConfig,
        project_root: &std::sync::Arc<std::sync::Mutex<PathBuf>>,
    ) -> LspManager {
        let mut manager = LspManager::new(config);
        let project_root = std::sync::Arc::clone(project_root);
        manager.set_workspace_trust(Box::new(move |root| {
            let project = project_root
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            workspace_approvals(&project).workspace_trusted(root)
        }));
        manager
    }

    /// Create LSP configuration from app config
    pub(crate) fn create_lsp_config(config: &Config) -> LspConfig {
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
        self.ui.options_submenu.open_unchosen();
    }

    /// Open Projects submenu
    pub fn open_projects_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.projects_submenu.open_unchosen();
        self.load_known_projects();
    }

    /// Open Tools submenu
    pub fn open_tools_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.tools_submenu.open_unchosen();
    }

    /// Open Commands submenu
    pub fn open_commands_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.commands_submenu.open_unchosen();
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
        self.ui.ai_submenu.open_unchosen();
    }

    /// Open the AI nested submenu for `section` (the section's item list).
    pub fn open_ai_nested_submenu(&mut self, section: termide_state::AiSection) {
        self.ui.ai_nested.open();
        self.ui.current_ai_section = Some(section);
        if section == termide_state::AiSection::Sessions {
            self.load_session_locations();
        }
    }

    /// Read the directories agent sessions are filed for.
    pub(crate) fn load_session_locations(&mut self) {
        self.cache.session_locations = ai_sessions_root()
            .map(|root| termide_agent_core::relocate::session_locations(&root))
            .unwrap_or_default();
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
        .with_home(dirs::home_dir().as_deref())
    }

    /// The Sessions section's rows: the directories sessions worked in,
    /// newest first, each led like a Projects menu row — a mark (✗ for a
    /// directory gone), then dimmed the time of its newest session and how
    /// many it has — and named by its path, losing its start when too long.
    fn ai_session_items(&self) -> Vec<termide_ui_render::DropdownItem> {
        use termide_ui_render::DropdownItem;
        let locations = &self.cache.session_locations;
        if locations.is_empty() {
            return vec![DropdownItem::new(
                termide_i18n::t().ai_empty(),
                String::new(),
            )];
        }
        let width = locations
            .iter()
            .map(|location| location.count.to_string().len())
            .max()
            .unwrap_or(1);
        locations
            .iter()
            .map(|location| {
                let mark = if location.cwd.is_dir() {
                    " "
                } else {
                    crate::projects_menu::MISSING_MARK
                };
                let time = location
                    .modified
                    .map(termide_project::format_local_minute)
                    .unwrap_or_default();
                DropdownItem::new(
                    termide_core::util::shorten_home_path(&location.cwd.display().to_string()),
                    format!("{SESSION_LOCATION_KEY}{}", location.dir.to_string_lossy()),
                )
                .with_prefix(
                    format!("{mark} "),
                    format!("{time} {:>width$}  ", location.count),
                )
                .cut_at_start()
            })
            .collect()
    }

    /// Build one AI section's dropdown rows. Called by both the renderer and the
    /// action handler so they address the same rows by index. Agents, skills and prompts get
    /// two "New …" rows and a separator before the merged, source-marked items
    /// (project-local first, in bold); sessions list the directories sessions
    /// worked in, project or not.
    pub fn ai_section_items(
        &self,
        section: termide_state::AiSection,
    ) -> Vec<termide_ui_render::DropdownItem> {
        use termide_state::AiSection;
        use termide_ui_render::DropdownItem;
        let t = termide_i18n::t();

        let dirs = self.ai_dirs();
        // (name, description, is_project, icon); only agents have an icon.
        let mut listed: Vec<(String, String, bool, Option<String>)> = match section {
            AiSection::Sessions => return self.ai_session_items(),
            AiSection::Agents => dirs
                .agents()
                .into_iter()
                .map(|name| {
                    let is_project = dirs
                        .agent_dir(&name)
                        .map(|p| p.starts_with(&self.project_root))
                        .unwrap_or(false);
                    let spec = dirs.spec(&name);
                    let icon = spec
                        .icon
                        .unwrap_or_else(|| termide_panel_agent::AGENT_ICON.to_string());
                    (name, spec.description, is_project, Some(icon))
                })
                .collect(),
            AiSection::Skills => dirs
                .skills()
                .into_iter()
                .map(|s| {
                    let is_project = s.path.starts_with(&self.project_root);
                    let description = termide_panel_agent::user_skill_description(&s);
                    (s.name, description, is_project, None)
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
                    (p.name, p.description, is_project, None)
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
        let has_project = listed.iter().any(|(_, _, p, _)| *p);
        let has_global = listed.iter().any(|(_, _, p, _)| !*p);
        let mut pushed_sep = false;
        for (name, description, is_project, icon) in listed {
            if !is_project && has_project && has_global && !pushed_sep {
                items.push(DropdownItem::separator());
                pushed_sep = true;
            }
            // The key keeps the bare name; the label adds the one-line
            // description the same way the panel's pickers show it.
            let description = description.lines().next().unwrap_or("").trim();
            let mut label = String::new();
            // The agent's icon only where the panel headers show icons.
            if let Some(icon) = icon.filter(|_| termide_core::terminal_caps::use_emoji_icons()) {
                label.push_str(&icon);
                label.push(' ');
            }
            label.push_str(&name);
            if !description.is_empty() {
                label.push_str(" · ");
                label.push_str(description);
            }
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

    /// Whether a question is on screen: a modal whose answer a
    /// `pending_action` waits for.
    pub fn is_asking(&self) -> bool {
        self.active_modal.is_some() && self.pending_action.is_some()
    }

    /// Ask a question nobody just asked for: one a panel raised in the
    /// background. Over an open question it waits its turn instead of
    /// replacing it — which would lose that question, what was typed into
    /// it and the answer its panel waits for. Otherwise it shows now, as
    /// [`Self::set_pending_action`] does. Returns whether it shows now.
    pub fn ask_when_free(&mut self, action: PendingAction, modal: ActiveModal) -> bool {
        if self.is_asking() || !self.queued_questions.is_empty() {
            self.queued_questions.push_back((action, modal));
            log::debug!(
                "A question is open; queued another behind it ({} waiting)",
                self.queued_questions.len()
            );
            false
        } else {
            self.resource_modal_kind = None;
            self.set_pending_action(action, modal);
            true
        }
    }

    /// Give the screen to the next waiting question, then to the next
    /// waiting report, if the screen is clear. Returns whether it did.
    pub fn deliver_queued_modal(&mut self) -> bool {
        if self.active_modal.is_some() {
            return false;
        }
        if let Some((action, modal)) = self.queued_questions.pop_front() {
            self.resource_modal_kind = None;
            self.set_pending_action(action, modal);
            return true;
        }
        self.deliver_queued_message_modal()
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

    /// Set error message. Nothing on screen draws the status message, so it
    /// goes to the journal too, where it can be read.
    pub fn set_error(&mut self, message: String) {
        log::warn!("{message}");
        self.ui.status_message = Some((message, true));
    }

    /// Set informational message, written to the journal too.
    pub fn set_info(&mut self, message: String) {
        log::info!("{message}");
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

    /// Save bookmarks to data directory; a failure shows on the status bar,
    /// since the change made in the menu is then gone at the next start.
    pub fn save_bookmarks(&mut self) {
        if let Err(e) = self.bookmarks.save() {
            self.set_error(termide_i18n::t().bookmarks_save_failed_fmt(&e.to_string()));
        }
    }

    /// Open bookmarks submenu
    pub fn open_bookmarks_submenu(&mut self) {
        self.ui.close_all_submenus();
        self.ui.bookmarks_submenu.open_unchosen();
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
        AppState::set_info(self, msg);
    }

    fn set_error(&mut self, msg: String) {
        AppState::set_error(self, msg);
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

    fn host_key(fingerprint: &str) -> PendingAction {
        PendingAction::HostKey {
            url: "sftp://h/".to_string(),
            fingerprint: fingerprint.to_string(),
        }
    }

    /// A question a panel raises in the background must not replace the
    /// one on screen: that one's answer — and its panel waiting on it —
    /// would be lost. It waits, then shows once the screen clears, each
    /// waiting question in turn, ahead of the waiting reports.
    #[test]
    fn a_background_question_waits_behind_an_open_one() {
        let mut s = state();
        s.set_pending_action(PendingAction::QuitApplication, question("Quit?"));

        assert!(!s.ask_when_free(host_key("first"), question("Trust first?")));
        assert!(!s.ask_when_free(host_key("second"), question("Trust second?")));
        s.show_message_modal(report("build"));

        assert!(matches!(
            s.pending_action,
            Some(PendingAction::QuitApplication)
        ));
        assert!(format!("{:?}", s.active_modal()).contains("Quit?"));
        assert!(!s.deliver_queued_modal(), "the screen is not free yet");

        s.close_modal();
        s.pending_action = None;
        assert!(s.deliver_queued_modal());
        assert!(
            matches!(&s.pending_action, Some(PendingAction::HostKey { fingerprint, .. }) if fingerprint == "first")
        );
        assert!(!s.deliver_queued_modal(), "one question at a time");

        s.close_modal();
        s.pending_action = None;
        assert!(s.deliver_queued_modal());
        assert!(
            matches!(&s.pending_action, Some(PendingAction::HostKey { fingerprint, .. }) if fingerprint == "second")
        );

        s.close_modal();
        s.pending_action = None;
        assert!(s.deliver_queued_modal(), "the report comes last");
        assert!(format!("{:?}", s.active_modal()).contains("build"));
        assert!(s.queued_questions.is_empty() && s.queued_info_modals.is_empty());
    }

    /// With no question open a background question shows at once, over a
    /// plain report as before.
    #[test]
    fn a_background_question_with_none_open_shows_at_once() {
        let mut s = state();
        s.show_message_modal(report("build"));

        assert!(s.ask_when_free(host_key("k"), question("Trust?")));
        assert!(matches!(
            s.pending_action,
            Some(PendingAction::HostKey { .. })
        ));
        assert!(s.queued_questions.is_empty());
    }
}

#[cfg(test)]
mod session_location_tests {
    use super::{AppState, SESSION_LOCATION_KEY};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};
    use termide_agent_core::relocate::SessionLocation;
    use termide_state::AiSection;

    fn state() -> AppState {
        let mut config = termide_config::Config::default();
        config.normalize();
        let theme = termide_theme::Theme::get_by_name(&config.general.theme);
        AppState::with_config_and_theme(config.clone(), config, theme)
    }

    fn location(cwd: PathBuf, count: usize) -> SessionLocation {
        SessionLocation {
            dir: PathBuf::from("/store").join(cwd.strip_prefix("/").unwrap_or(&cwd)),
            cwd,
            count,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(60)),
        }
    }

    /// A Sessions row leads with a mark — ✗ for a directory gone — then
    /// dims the time and the count, right-aligned to the widest, before
    /// the path; its key names where the sessions are filed.
    #[test]
    fn a_location_row_dims_its_time_and_count_before_the_path() {
        let here = tempfile::tempdir().unwrap();
        let gone = PathBuf::from("/nonexistent/termide/gone");
        let mut state = state();
        state.cache.session_locations = vec![
            location(here.path().to_path_buf(), 519),
            location(gone.clone(), 5),
        ];

        let items = state.ai_section_items(AiSection::Sessions);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].lead, "  ");
        assert!(items[0].muted.ends_with(" 519  "), "{:?}", items[0].muted);
        assert_eq!(
            items[1].lead,
            format!("{} ", crate::projects_menu::MISSING_MARK)
        );
        assert!(items[1].muted.ends_with("   5  "), "{:?}", items[1].muted);
        assert_eq!(items[1].label, gone.display().to_string());
        assert_eq!(
            items[1].key,
            format!("{SESSION_LOCATION_KEY}/store/nonexistent/termide/gone")
        );
    }

    /// With no sessions anywhere the section says it is empty.
    #[test]
    fn no_locations_is_an_empty_section() {
        let items = state().ai_section_items(AiSection::Sessions);
        assert_eq!(items.len(), 1);
        assert!(items[0].key.is_empty());
    }
}
