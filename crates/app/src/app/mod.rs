//! Main application module.
//!
//! Contains the App struct and all event handlers.

// Note: PanelExt is used for panel-specific operations (get current path, save editor)
// that require concrete type access. Common operations use Panel::handle_command().
#![allow(deprecated)]

use anyhow::Result;
use ratatui::{backend::Backend, Terminal};
use std::str::FromStr;
use std::time::Duration;

use crossterm::event::MouseEventKind;
use termide_app_core::{
    ActiveModal as CoreActiveModal, LayoutController, ModalManager, PanelProvider,
    PendingAction as CorePendingAction, StateManager, UiState,
};
use termide_core::event::{Event, EventHandler};
use termide_core::PanelCommand;
use termide_layout::{LayoutManager, PanelGroup};

use termide_config::Config;
use termide_theme::Theme;
use termide_ui::ClickTracker;

use crate::state::AppState;
use crate::PanelExt;

// Panel trait re-export
pub use termide_core::Panel;

mod background_ops;
mod bg_fetch;
mod bg_git;
mod bg_lsp;
mod bg_resource;
mod event_handler;
mod file_transfer_ops;
mod global_hotkeys;
mod key_handler;
mod local_ops;
mod menu_actions;
mod modal;
mod modal_handler;
mod mouse;
mod mouse_handler;
mod operation_manager_handler;
mod panel_factory;
mod panel_manager;
mod panel_operations;
mod session;
mod watcher;
mod workspace_edit;

/// Main application
pub struct App {
    state: AppState,
    layout_manager: LayoutManager,
    event_handler: EventHandler,
    /// Project root directory (used for per-project session storage)
    project_root: std::path::PathBuf,
    /// Last seen editor edit_version (for debounced outline sync).
    outline_last_version: u64,
    /// Last seen editor cursor line (for outline cursor sync).
    outline_last_cursor: usize,
    /// Timestamp of last outline content update (debounce 1s).
    outline_last_edit_time: Option<std::time::Instant>,
    /// Click tracker for double-click on panel title (directory picker).
    title_click_tracker: ClickTracker<(u16, u16)>,
    /// Cached command list for Command Palette (index → action name).
    command_palette_actions: Option<Vec<String>>,
    /// Capability-aware key normalizer. Used to build a `KeyChord` from
    /// raw `KeyEvent`s on the dispatch boundary so that all matchers
    /// downstream see canonical keys while text-input and PTY paths
    /// keep the original raw event.
    normalizer: termide_keyboard::KeyNormalizer,
    /// Whether the session is persisted. Disabled when termide is launched
    /// with explicit file arguments ($EDITOR mode), so editing a commit
    /// message or crontab never restores or overwrites the project session.
    persist_session: bool,
    /// Focus signature (group index, active panel name) from the previous
    /// input, used to detect when focus moves to a git panel so it can be
    /// refreshed automatically (no manual Ctrl+R).
    last_focus_sig: Option<(usize, String)>,
}

impl App {
    /// Create a new application
    pub fn new() -> Self {
        let mut state = AppState::new();

        // Get project root from current working directory
        let project_root = std::env::current_dir().unwrap_or_else(|_| {
            // Fallback to home directory if current_dir fails
            dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"))
        });

        // Load project-local bookmarks from .termide/ if present
        state.project_root = project_root.clone();
        state.project_bookmarks = termide_config::BookmarksConfig::load_from_project(&project_root);

        // Initialize logger in session directory (before other initializations that log)
        // Use config override if specified, otherwise use session directory with unique filename
        let log_file_path = if let Some(ref path) = state.config.logging.file_path {
            std::path::PathBuf::from(path)
        } else {
            termide_session::Session::get_session_dir(&project_root)
                .map(|dir| {
                    // Cleanup old log files (older than 24 hours)
                    let _ = termide_session::cleanup_old_logs(&dir);
                    dir.join(termide_session::generate_log_filename())
                })
                .unwrap_or_else(|_| {
                    std::env::temp_dir().join(termide_session::generate_log_filename())
                })
        };
        let min_log_level = termide_logger::LogLevel::from_str(&state.config.logging.min_level)
            .ok()
            .unwrap_or(termide_logger::LogLevel::Info);
        termide_logger::init(
            log_file_path,
            termide_config::constants::MAX_LOG_ENTRIES,
            min_log_level,
        );
        log::info!("Application started");

        // Initialize unified watcher for filesystem and git events
        match termide_watcher::create_watcher() {
            Ok(watcher) => {
                state.watcher = Some(watcher);
                log::info!("Unified watcher initialized");
            }
            Err(e) => {
                log::error!("Failed to initialize watcher: {}", e);
            }
        }

        // Clean up old sessions (configurable retention period)
        let retention_days = state.config.general.session_retention_days;
        if let Err(e) = termide_session::cleanup_old_sessions(&project_root, retention_days) {
            log::warn!("Failed to cleanup old sessions: {}", e);
        }

        Self {
            state,
            layout_manager: LayoutManager::new(),
            event_handler: EventHandler::new(Duration::from_millis(
                termide_config::constants::EVENT_HANDLER_INTERVAL_MS,
            )),
            project_root,
            outline_last_version: 0,
            outline_last_cursor: 0,
            outline_last_edit_time: None,
            title_click_tracker: ClickTracker::new(),
            command_palette_actions: None,
            normalizer: termide_keyboard::KeyNormalizer::default(),
            persist_session: true,
            last_focus_sig: None,
        }
    }

    /// Create a new application with a pre-loaded config and specified terminal size.
    /// This avoids double config loading by accepting an already-configured Config.
    ///
    /// `caps` carries the keyboard-protocol capabilities detected at
    /// startup (Kitty enhancement flags). The `KeyNormalizer` reads
    /// them so quirk fixes (VTE Ctrl+7, caps-lock strip) apply only on
    /// terminals where they make sense.
    pub fn new_with_config(
        config: Config,
        global_baseline: Config,
        width: u16,
        height: u16,
        caps: termide_keyboard::KeyboardCaps,
    ) -> Self {
        let theme = Theme::get_by_name(&config.general.theme);
        let mut state = AppState::with_config_and_theme(config, global_baseline, theme);
        state.update_terminal_size(width, height);

        // Get project root from current working directory
        let project_root = std::env::current_dir()
            .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/")));

        // Initialize logger in session directory
        let log_file_path = if let Some(ref path) = state.config.logging.file_path {
            std::path::PathBuf::from(path)
        } else {
            termide_session::Session::get_session_dir(&project_root)
                .map(|dir| {
                    let _ = termide_session::cleanup_old_logs(&dir);
                    dir.join(termide_session::generate_log_filename())
                })
                .unwrap_or_else(|_| {
                    std::env::temp_dir().join(termide_session::generate_log_filename())
                })
        };
        let min_log_level = termide_logger::LogLevel::from_str(&state.config.logging.min_level)
            .ok()
            .unwrap_or(termide_logger::LogLevel::Info);
        termide_logger::init(
            log_file_path,
            termide_config::constants::MAX_LOG_ENTRIES,
            min_log_level,
        );
        log::info!("Application started");

        // Initialize unified watcher for filesystem and git events
        match termide_watcher::create_watcher() {
            Ok(watcher) => {
                state.watcher = Some(watcher);
                log::info!("Unified watcher initialized");
            }
            Err(e) => {
                log::error!("Failed to initialize watcher: {}", e);
            }
        }

        // Clean up old sessions
        let retention_days = state.config.general.session_retention_days;
        if let Err(e) = termide_session::cleanup_old_sessions(&project_root, retention_days) {
            log::warn!("Failed to cleanup old sessions: {}", e);
        }

        // Warn about same-section conflicts and bindings that need
        // Kitty keyboard protocol (when this terminal does not advertise
        // it). Logs only — config is left as-is per user choice.
        Self::log_keybinding_warnings(&state.config, caps);

        Self {
            state,
            layout_manager: LayoutManager::new(),
            event_handler: EventHandler::new(Duration::from_millis(
                termide_config::constants::EVENT_HANDLER_INTERVAL_MS,
            )),
            project_root,
            outline_last_version: 0,
            outline_last_cursor: 0,
            outline_last_edit_time: None,
            title_click_tracker: ClickTracker::new(),
            command_palette_actions: None,
            normalizer: termide_keyboard::KeyNormalizer::new(caps),
            persist_session: true,
            last_focus_sig: None,
        }
    }

    /// Enable or disable session persistence. Disabled for `$EDITOR`-style
    /// launches (explicit file arguments) so the project session is neither
    /// restored nor overwritten.
    pub fn set_session_persistence(&mut self, enabled: bool) {
        self.persist_session = enabled;
    }

    /// Open a file path in a new editor panel at startup (used for CLI file
    /// arguments). Creates the file (and parent directories) if it does not
    /// exist yet, so termide works as `$EDITOR` for new files too.
    pub fn open_path_in_editor(&mut self, path: std::path::PathBuf) -> Result<()> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::File::create(&path)?;
        }
        self.open_editor_for_file(path)
    }

    /// Log same-section conflicts and bindings that need Kitty
    /// keyboard protocol on terminals that don't advertise it.
    fn log_keybinding_warnings(config: &Config, caps: termide_keyboard::KeyboardCaps) {
        let conflicts = termide_config::find_conflicts(config);
        for c in conflicts
            .iter()
            .filter(|c| c.kind == termide_config::ConflictKind::SameSection)
        {
            let where_str = c
                .locations
                .iter()
                .map(|l| l.display())
                .collect::<Vec<_>>()
                .join(", ");
            log::warn!(
                "Keybinding {} is assigned to multiple actions in the same section: {}",
                c.binding,
                where_str,
            );
        }

        // macOS: Option composes text unless REPORT_ALL_KEYS_AS_ESCAPE_CODES is
        // active, so every `Alt+<char>` chord arrives as a composed glyph
        // (`Option+F` → `ƒ`) with no ALT bit and simply never fires. Say so,
        // with the remedy that applies to the terminal at hand.
        if cfg!(target_os = "macos") && !caps.all_keys {
            let affected = termide_config::enumerate_bindings(config)
                .into_iter()
                .filter(|(_, parsed, _)| termide_config::binding_requires_macos_all_keys(parsed))
                .count();
            if affected > 0 {
                let remedy = if caps.kitty_full {
                    "set general.report_all_keys = true in config.toml"
                } else {
                    "this terminal has no Kitty keyboard protocol; enable its own                      Option-as-Alt setting (Ghostty: macos-option-as-alt,                      Terminal.app: Use Option as Meta key)"
                };
                log::warn!(
                    "{affected} Alt+<key> keybinding(s) cannot fire on macOS because                      Option is composing text instead: {remedy}.",
                );
            }
        }

        if !caps.kitty_full {
            let problematic: Vec<_> = termide_config::enumerate_bindings(config)
                .into_iter()
                .filter(|(_, parsed, _)| termide_config::binding_requires_kitty(parsed))
                .collect();
            if !problematic.is_empty() {
                let mut sample: Vec<String> = problematic
                    .iter()
                    .take(5)
                    .map(|(loc, _, raw)| format!("{} ({})", raw, loc.display()))
                    .collect();
                if problematic.len() > sample.len() {
                    sample.push(format!("…and {} more", problematic.len() - sample.len()));
                }
                log::warn!(
                    "{} keybinding(s) require Kitty keyboard protocol; this terminal \
                     does not advertise it. Affected: {}",
                    problematic.len(),
                    sample.join(", "),
                );
            }
        }
    }

    /// Persist `config` to the currently-active config target.
    ///
    /// Active target is derived from the existence of
    /// `<project_root>/.termide/config.toml`:
    /// - **Exists** → save the project-level diff against
    ///   `state.global_baseline`. Only fields that differ from
    ///   `defaults + global` survive in the project file.
    /// - **Absent** → save the global-level diff against
    ///   `Config::default()`. The user's `~/.config/termide/config.toml`
    ///   keeps only fields they have explicitly changed.
    ///
    /// On success the in-memory `state.config` is replaced with the
    /// new value so callers and panels see the change immediately.
    pub fn save_config_to_active_target(&mut self, config: Config) -> anyhow::Result<()> {
        let project_path = termide_config::project_config_path(&self.project_root);
        if project_path.exists() {
            config.save_project(&self.project_root, &self.state.global_baseline)?;
        } else {
            config.save_global()?;
        }
        self.state.config = std::sync::Arc::new(config);
        Ok(())
    }

    /// Log git availability status to journal
    pub fn log_git_status(&self, git_available: bool) {
        if git_available {
            log::info!("Git detected and available");
        } else {
            log::warn!("Git not found - git integration disabled");
        }
    }

    /// Add a panel (automatically stacks if width threshold is reached)
    pub fn add_panel(&mut self, mut panel: Box<dyn Panel>) {
        use termide_core::PanelCommand;

        // Notify new panel about current git operation state
        if self.state.ui.git_operation_in_progress {
            let operation = self
                .state
                .git_operation_handle
                .as_ref()
                .map(|h| h.operation.clone());
            panel.handle_command(PanelCommand::SetGitOperationInProgress {
                in_progress: true,
                operation,
                spinner_frame: self.state.ui.spinner_frame,
            });
        }

        let terminal_width = self.state.terminal.width;
        let config = &self.state.config;
        self.layout_manager.add_panel(panel, config, terminal_width);
        self.state.needs_watcher_registration = true;
    }

    /// Add panel without changing focus.
    /// Used for preview panels where focus should stay on the source panel.
    pub fn add_panel_without_focus(&mut self, mut panel: Box<dyn Panel>) {
        use termide_core::PanelCommand;

        // Notify new panel about current git operation state
        if self.state.ui.git_operation_in_progress {
            let operation = self
                .state
                .git_operation_handle
                .as_ref()
                .map(|h| h.operation.clone());
            panel.handle_command(PanelCommand::SetGitOperationInProgress {
                in_progress: true,
                operation,
                spinner_frame: self.state.ui.spinner_frame,
            });
        }

        let terminal_width = self.state.terminal.width;
        let config = &self.state.config;
        self.layout_manager
            .add_panel_without_focus(panel, config, terminal_width);
        self.state.needs_watcher_registration = true;
    }

    /// Setup default layout based on terminal width.
    ///
    /// | Width   | Groups | Layout                                                  |
    /// |---------|--------|---------------------------------------------------------|
    /// | 1-99    | 1      | [*: FM]                                                 |
    /// | 100-139 | 2      | [*: FM] + [40: FM, GitStatus?]                          |
    /// | 140-199 | 3      | [*: FM] + [40: FM] + [40: GitStatus?, Outline]          |
    /// | 200+    | 3      | [*: FM] + [*: FM] + [40: GitStatus?, Outline]           |
    ///
    /// `*` = remaining space. GitStatus added only if a git repo is detected.
    pub fn setup_default_layout(&mut self) {
        use termide_panel_file_manager::FileManager;

        let width = self.state.terminal.width;
        let repo_root = termide_git::find_repo_root(&self.project_root);

        if width < 100 {
            // 1 group: [*: FM]
            let g1 = PanelGroup::new(Box::new(FileManager::new()));
            self.layout_manager.panel_groups.push(g1);
            self.layout_manager.focus = 0;
        } else if width < 140 {
            // 2 groups: [*: FM] + [40: FM, GitStatus?]
            let g1 = PanelGroup::new(Box::new(FileManager::new()));

            let mut g2 = PanelGroup::new(Box::new(FileManager::new()));
            if let Some(repo) = repo_root {
                g2.add_panel(Box::new(
                    termide_panel_git_status::GitStatusPanel::new_for_repo(repo),
                ));
            }
            g2.width = Some(40);

            self.layout_manager.panel_groups.push(g1);
            self.layout_manager.panel_groups.push(g2);
            self.layout_manager.focus = 0;
        } else if width < 200 {
            // 3 groups: [*: FM] + [40: FM] + [40: GitStatus?, Outline]
            let g1 = PanelGroup::new(Box::new(FileManager::new()));

            let mut g2 = PanelGroup::new(Box::new(FileManager::new()));
            g2.width = Some(40);

            let mut g3 = if let Some(repo) = repo_root {
                let mut g = PanelGroup::new(Box::new(
                    termide_panel_git_status::GitStatusPanel::new_for_repo(repo),
                ));
                g.add_panel(Box::new(termide_panel_outline::OutlinePanel::new(
                    *self.state.theme,
                )));
                g
            } else {
                PanelGroup::new(Box::new(termide_panel_outline::OutlinePanel::new(
                    *self.state.theme,
                )))
            };
            g3.width = Some(40);

            self.layout_manager.panel_groups.push(g1);
            self.layout_manager.panel_groups.push(g2);
            self.layout_manager.panel_groups.push(g3);
            self.layout_manager.focus = 0;
        } else {
            // 200+: [*: FM] + [*: FM] + [40: GitStatus?, Outline]
            let g1 = PanelGroup::new(Box::new(FileManager::new()));

            let g2 = PanelGroup::new(Box::new(FileManager::new()));

            let mut g3 = if let Some(repo) = repo_root {
                let mut g = PanelGroup::new(Box::new(
                    termide_panel_git_status::GitStatusPanel::new_for_repo(repo),
                ));
                g.add_panel(Box::new(termide_panel_outline::OutlinePanel::new(
                    *self.state.theme,
                )));
                g
            } else {
                PanelGroup::new(Box::new(termide_panel_outline::OutlinePanel::new(
                    *self.state.theme,
                )))
            };
            g3.width = Some(40);

            self.layout_manager.panel_groups.push(g1);
            self.layout_manager.panel_groups.push(g2);
            self.layout_manager.panel_groups.push(g3);
            self.layout_manager.focus = 0;
        }

        self.state.needs_watcher_registration = true;
    }

    /// Run the main application loop
    /// When input moves focus to a git panel, reload it so changes made
    /// outside the panel appear automatically (no manual Ctrl+R). Detected by
    /// comparing a (focus group, active panel name) signature across inputs.
    fn refresh_git_panel_on_focus_change(&mut self) {
        let sig = self
            .layout_manager
            .active_panel()
            .map(|p| (self.layout_manager.focus, p.name().to_string()));
        if sig == self.last_focus_sig {
            return;
        }
        self.last_focus_sig = sig.clone();
        if let Some((_, name)) = sig {
            if name == "git_status" || name == "git_diff" || name == "git_log" {
                if let Some(panel) = self.layout_manager.active_panel_mut() {
                    if panel.handle_command(PanelCommand::Reload).needs_redraw() {
                        self.state.needs_redraw = true;
                    }
                }
            }
        }
    }

    pub fn run<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        render_fn: impl Fn(&mut ratatui::Frame<'_>, &mut AppState, &mut LayoutManager),
    ) -> Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        // Initialize terminal dimensions
        let size = terminal.size()?;
        self.state.update_terminal_size(size.width, size.height);

        while !self.state.should_quit {
            // Process events
            match self.event_handler.next()? {
                Event::Key(key) => {
                    self.state.last_activity = std::time::Instant::now();
                    self.event_handler.set_tick_rate(Duration::from_millis(
                        termide_config::constants::EVENT_HANDLER_INTERVAL_MS,
                    ));
                    self.handle_key_event(key)?;
                    self.refresh_git_panel_on_focus_change();
                    self.state.needs_redraw = true;
                }
                Event::Mouse(mouse) => {
                    let is_moved = matches!(mouse.kind, MouseEventKind::Moved);
                    // Mouse movement (hover) should not wake the app from idle,
                    // but the active terminal panel still needs to see it for
                    // mouse-tracking passthrough and Ctrl+hover visuals.
                    if !is_moved {
                        self.state.last_activity = std::time::Instant::now();
                        self.event_handler.set_tick_rate(Duration::from_millis(
                            termide_config::constants::EVENT_HANDLER_INTERVAL_MS,
                        ));
                    }
                    self.handle_mouse_event(mouse)?;
                    self.refresh_git_panel_on_focus_change();
                    if !is_moved {
                        self.state.needs_redraw = true;
                    }
                }
                Event::Resize(width, height) => {
                    // Update terminal dimensions in state
                    self.state.update_terminal_size(width, height);

                    // Пропорционально перераспределить ширины групп при изменении размера терминала
                    self.layout_manager
                        .redistribute_widths_proportionally(width);
                    self.state.needs_redraw = true;
                }
                Event::FocusLost => {
                    self.notify_active_panel_host_focus(false);
                    // Save session on focus loss (with debounce)
                    if self.state.should_save_session() {
                        self.auto_save_session();
                        self.state.update_last_session_save();
                    }
                }
                Event::FocusGained => {
                    self.notify_active_panel_host_focus(true);
                    // Redraw on focus gain to refresh display
                    self.state.needs_redraw = true;
                }
                Event::Paste(text) => {
                    self.state.last_activity = std::time::Instant::now();
                    self.event_handler.set_tick_rate(Duration::from_millis(
                        termide_config::constants::EVENT_HANDLER_INTERVAL_MS,
                    ));
                    // Handle bracketed paste - check modal first, then send to active panel
                    if !self.handle_modal_paste(&text) {
                        self.handle_paste_event(text)?;
                    }
                    self.state.needs_redraw = true;
                }
                Event::MouseScrollCoalesced { event, delta } => {
                    self.state.last_activity = std::time::Instant::now();
                    self.event_handler.set_tick_rate(Duration::from_millis(
                        termide_config::constants::EVENT_HANDLER_INTERVAL_MS,
                    ));
                    // Handle coalesced scroll events (batched for performance)
                    self.handle_coalesced_scroll(event, delta)?;
                    self.state.needs_redraw = true;
                }
                Event::Tick => self.poll_background(),
            }

            // Check and close panels that should auto-close
            self.check_auto_close_panels()?;

            // Render UI only when needed (reduces idle CPU from 24fps to near-zero)
            if self.state.needs_redraw {
                terminal.draw(|frame| {
                    render_fn(frame, &mut self.state, &mut self.layout_manager);
                })?;
                self.state.needs_redraw = false;
            }
        }

        Ok(())
    }

    /// Per-tick background polling batch, run once for every `Event::Tick`.
    ///
    /// Keeps `run` as pure loop control: this drains terminal output and panel
    /// ticks, honours the scroll/drag fast-paths, then fans out to the watcher,
    /// operation-manager, git/command/fetch pollers and the always-on
    /// system-resource / LSP-completion / modal-spinner updates.
    fn poll_background(&mut self) {
        // Adaptive tick rate: slow down polling when idle
        if self.state.last_activity.elapsed()
            > Duration::from_millis(termide_config::constants::IDLE_THRESHOLD_MS)
        {
            self.event_handler.set_tick_rate(Duration::from_millis(
                termide_config::constants::IDLE_TICK_MS,
            ));
        }

        // Debounce scroll renders: consume pending flag and trigger redraw
        if self.state.pending_scroll_render {
            self.state.pending_scroll_render = false;
            self.state.needs_redraw = true;
        }

        // Detect active scrolling (within 100ms of last scroll event)
        // Skip heavy operations during scrolling to prevent UI lag
        let is_scrolling = self
            .state
            .last_mouse_scroll
            .map(|t| t.elapsed() < Duration::from_millis(100))
            .unwrap_or(false);

        let is_dragging = self.state.ui.drag.is_dragging();

        // Skip heavy operations during active scrolling or divider drag
        if !is_scrolling && !is_dragging {
            // Single combined loop: terminal output + panel tick + FM spinner
            let mut all_panel_events = Vec::new();
            for (panel, is_expanded) in self
                .layout_manager
                .iter_all_panels_with_expanded_state_mut()
            {
                // Terminal output check (always needed, even during idle)
                // PTY must be drained to avoid buffer deadlock
                if let Some(terminal) = panel.as_terminal_mut() {
                    if terminal.has_pending_output() {
                        self.state.needs_redraw = true;
                    }
                }

                // Always call tick() — stale panels drain async
                // results internally and return early
                let events = panel.tick();
                if !events.is_empty() {
                    self.state.needs_redraw = true;
                    all_panel_events.extend(events);
                }

                // FileManager-specific: only check VFS for expanded panels
                if is_expanded {
                    if let Some(fm) = panel.as_file_manager_mut() {
                        if fm.vfs_state().has_pending_operation() {
                            self.state.needs_redraw = true;
                        }
                    }
                }
            }
            // Process collected events
            if !all_panel_events.is_empty() {
                if let Err(e) = self.process_panel_events(all_panel_events) {
                    log::error!("Error processing panel events: {}", e);
                }
            }
        } else {
            // During scrolling: only check terminal output (lightweight)
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(terminal) = panel.as_terminal_mut() {
                    if terminal.has_pending_output() {
                        self.state.needs_redraw = true;
                        break;
                    }
                }
            }
        }

        if !is_scrolling {
            // Check for modal requests from FileManager panels (e.g., VFS error modals)
            // This must happen after tick() processing to show connection error modals
            let modal_request = self
                .layout_manager
                .iter_all_panels_mut()
                .find_map(|panel| panel.take_modal_request());
            if let Some((action, modal)) = modal_request {
                if let Err(e) = self.handle_modal_request(action, modal) {
                    log::error!("Error handling modal request: {}", e);
                }
                self.state.needs_redraw = true;
            }

            // Check for pending upload operations from Editor panels (Ctrl+S remote saves)
            let pending_upload = self
                .layout_manager
                .iter_all_panels_mut()
                .find_map(|panel| panel.take_pending_upload());
            if let Some((temp_path, remote_path, vfs_manager)) = pending_upload {
                self.handle_pending_upload(temp_path, remote_path, vfs_manager);
                self.state.needs_redraw = true;
            }

            // Check channel for directory size calculation results
            self.check_dir_size_update();

            // Register panel watchers only when needed (panel added/navigated)
            if self.state.needs_watcher_registration {
                self.register_panel_watchers();
                self.state.needs_watcher_registration = false;
            }

            // Drain any finished repository walks so the
            // inotify watches install non-blockingly. The
            // walk itself runs on a worker thread; this is
            // just the per-path `watcher.watch` call.
            if let Some(watcher) = self.state.watcher.as_mut() {
                if watcher.poll_pending() {
                    self.state.needs_redraw = true;
                }
            }

            // Poll unified watcher for git and filesystem events
            self.poll_watcher_events();

            // Single-pass: async git status, async dir reload, pending git diff
            self.check_background_panel_updates();

            // Deliver a completed viewer URL fetch (Ctrl+G with a URL)
            self.check_view_fetch();

            // Check background git operation result (push/pull)
            self.check_git_operation_result();

            // Check background command operation result (.report. commands)
            self.check_command_operation_result();
            self.check_bg_command_completion();

            // Poll unified operation manager for events (new system)
            self.poll_operation_manager();

            // Sync active operations data to the operations panel
            self.update_operations_panel();

            // Debounced outline sync for live editing (cheap: u64 compare only)
            self.check_outline_live_edit();

            // Apply pending outline navigation to editor
            self.apply_outline_navigation();

            // Check pending local batch operation (start after modal rendered)
            self.check_pending_batch_operation();
        }

        // Update system resource monitoring (CPU, RAM)
        // This is fast, always run it
        self.update_system_resources();

        // Poll LSP completion responses for active editor
        // This is fast, always run it
        self.poll_lsp_completion();

        // Update spinner in all modals that support animation
        // This is fast, always run it
        self.update_modal_spinners();
    }

    /// Check and close panels that should auto-close
    fn check_auto_close_panels(&mut self) -> Result<()> {
        // Check if active panel should auto-close
        let should_close = {
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                panel.should_auto_close()
            } else {
                false
            }
        };

        if should_close && self.layout_manager.can_close_active() {
            self.state.bell();
            // Calculate available width for panel groups
            let terminal_width = self.state.terminal.width;

            let _ = self.layout_manager.close_active_panel(terminal_width);
            self.auto_save_session();
        }

        Ok(())
    }

    // ===== Panel downcast helpers =====
    // These helper methods reduce boilerplate for accessing specific panel types

    /// Get mutable reference to the active file manager panel (if any).
    fn active_file_manager_mut(&mut self) -> Option<&mut termide_panel_file_manager::FileManager> {
        let panel = self.layout_manager.active_panel_mut()?;
        panel.as_file_manager_mut()
    }

    /// Get reference to AppState
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Get reference to LayoutManager
    pub fn layout_manager(&self) -> &LayoutManager {
        &self.layout_manager
    }

    /// Find existing panel by name and focus on it
    /// Returns true if found and focused, false if not found
    fn find_and_focus_panel_by_name(&mut self, name: &str) -> bool {
        for (group_idx, group) in self.layout_manager.panel_groups.iter_mut().enumerate() {
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                if panel.name() == name {
                    self.layout_manager.focus = group_idx;
                    group.set_expanded(panel_idx);
                    return true;
                }
            }
        }
        false
    }

    /// Find an editor with the given file path and focus it. Returns true if found.
    #[allow(deprecated)]
    fn focus_editor_by_path(&mut self, path: &std::path::Path) -> bool {
        use crate::panel_ext::PanelExt;
        for (group_idx, group) in self.layout_manager.panel_groups.iter_mut().enumerate() {
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                if let Some(editor) = panel.as_editor() {
                    if editor.file_path() == Some(path) {
                        self.layout_manager.focus = group_idx;
                        group.set_expanded(panel_idx);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Show error as InfoModal; fall back to status bar if a modal is already active.
    pub(crate) fn show_error_modal(&mut self, message: String) {
        if self.state.has_modal() {
            self.state.set_error(message);
            return;
        }
        let lines = vec![(String::new(), message)];
        let modal = termide_modal::InfoModal::new(termide_i18n::t().modal_error_title(), lines);
        self.state.active_modal = Some(termide_modal::ActiveModal::Info(Box::new(modal)));
    }

    fn notify_active_panel_host_focus(&mut self, focused: bool) {
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if panel
                .handle_command(PanelCommand::SetHostFocus { focused })
                .needs_redraw()
            {
                self.state.needs_redraw = true;
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Core Trait Implementations
// ============================================================================

impl PanelProvider for App {
    fn active_panel(&self) -> Option<&dyn Panel> {
        self.layout_manager
            .active_panel()
            .map(|p| p.as_ref() as &dyn Panel)
    }

    fn active_panel_mut(&mut self) -> Option<&mut Box<dyn Panel>> {
        self.layout_manager.active_panel_mut()
    }

    fn active_panel_index(&self) -> Option<usize> {
        self.layout_manager.active_group_index()
    }

    fn panel_count(&self) -> usize {
        self.layout_manager.panel_count()
    }

    fn iter_panels_mut(&mut self) -> Box<dyn Iterator<Item = &mut Box<dyn Panel>> + '_> {
        Box::new(self.layout_manager.iter_all_panels_mut())
    }
}

impl StateManager for App {
    fn ui(&self) -> &UiState {
        &self.state.ui
    }

    fn ui_mut(&mut self) -> &mut UiState {
        &mut self.state.ui
    }

    fn set_info(&mut self, msg: String) {
        self.state.set_info(msg);
    }

    fn set_error(&mut self, msg: String) {
        self.state.set_error(msg);
    }

    fn clear_status(&mut self) {
        self.state.clear_status();
    }

    fn needs_redraw(&self) -> bool {
        self.state.needs_redraw
    }

    fn set_redraw(&mut self, value: bool) {
        self.state.needs_redraw = value;
    }
}

impl ModalManager for App {
    fn active_modal(&self) -> Option<&CoreActiveModal> {
        self.state.active_modal.as_ref()
    }

    fn active_modal_mut(&mut self) -> Option<&mut CoreActiveModal> {
        self.state.active_modal.as_mut()
    }

    fn open_modal(&mut self, modal: CoreActiveModal, action: Option<CorePendingAction>) {
        self.state.active_modal = Some(modal);
        self.state.pending_action = action;
    }

    fn close_modal(&mut self) {
        self.state.close_modal();
    }

    fn take_pending_action(&mut self) -> Option<CorePendingAction> {
        self.state.pending_action.take()
    }
}

impl LayoutController for App {
    fn add_panel(&mut self, panel: Box<dyn Panel>) {
        // Use the main add_panel method which handles git operation state notification
        App::add_panel(self, panel);
    }

    fn close_active(&mut self) -> Result<()> {
        let terminal_width = self.state.terminal.width;
        self.layout_manager.close_active_panel(terminal_width)
    }

    fn next_group(&mut self) {
        self.layout_manager.next_group();
    }

    fn prev_group(&mut self) {
        self.layout_manager.prev_group();
    }

    fn next_in_group(&mut self) {
        self.layout_manager.next_panel_in_group();
    }

    fn prev_in_group(&mut self) {
        self.layout_manager.prev_panel_in_group();
    }

    fn set_focus(&mut self, index: usize) {
        self.layout_manager.set_focus(index);
    }
}
