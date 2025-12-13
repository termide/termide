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

use termide_app_core::{LayoutController, PanelProvider};
use termide_app_event::DefaultHotkeyProcessor;
use termide_core::event::{Event, EventHandler};
use termide_layout::LayoutManager;

use crate::LayoutManagerSession;

use crate::state::AppState;
use crate::PanelExt;

// Panel trait re-export
pub use termide_core::Panel;

mod event_handler;
mod global_hotkeys;
mod key_handler;
mod menu_actions;
mod modal;
mod modal_handler;
mod mouse_handler;
mod panel_manager;
mod panel_operations;

/// Main application
pub struct App {
    state: AppState,
    layout_manager: LayoutManager,
    event_handler: EventHandler,
    /// Project root directory (used for per-project session storage)
    project_root: std::path::PathBuf,
    /// Global hotkey processor
    hotkey_processor: DefaultHotkeyProcessor,
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
        termide_logger::info("Application started");

        // Initialize git watcher for automatic status updates
        match termide_git::create_git_watcher() {
            Ok((watcher, receiver)) => {
                state.git_watcher = Some(watcher);
                state.git_watcher_receiver = Some(receiver);
                termide_logger::info("Git watcher initialized");
            }
            Err(e) => {
                termide_logger::error(format!("Failed to initialize git watcher: {}", e));
            }
        }

        // Initialize filesystem watcher for automatic directory updates
        match termide_watcher::create_fs_watcher() {
            Ok((watcher, receiver)) => {
                state.fs_watcher = Some(watcher);
                state.fs_watcher_receiver = Some(receiver);
                termide_logger::info("FS watcher initialized");
            }
            Err(e) => {
                termide_logger::error(format!("Failed to initialize FS watcher: {}", e));
            }
        }

        // Clean up old sessions (configurable retention period)
        let retention_days = state.config.general.session_retention_days;
        if let Err(e) = termide_session::cleanup_old_sessions(&project_root, retention_days) {
            termide_logger::warn(format!("Failed to cleanup old sessions: {}", e));
        }

        Self {
            state,
            layout_manager: LayoutManager::new(),
            event_handler: EventHandler::new(Duration::from_millis(
                termide_config::constants::EVENT_HANDLER_INTERVAL_MS,
            )),
            project_root,
            hotkey_processor: DefaultHotkeyProcessor::new(),
        }
    }

    /// Create a new application with specified terminal size
    /// This is useful during initialization to set proper terminal dimensions
    /// before creating panels
    pub fn new_with_size(width: u16, height: u16) -> Self {
        let mut app = Self::new();
        app.state.update_terminal_size(width, height);
        app
    }

    /// Add a panel (automatically stacks if width threshold is reached)
    pub fn add_panel(&mut self, panel: Box<dyn Panel>) {
        let terminal_width = self.state.terminal.width;
        let config = &self.state.config;
        self.layout_manager.add_panel(panel, config, terminal_width);
    }

    /// Run the main application loop
    pub fn run<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        render_fn: impl Fn(&mut ratatui::Frame<'_>, &mut AppState, &mut LayoutManager),
    ) -> Result<()> {
        // Initialize terminal dimensions
        let size = terminal.size()?;
        self.state.update_terminal_size(size.width, size.height);

        while !self.state.should_quit {
            // Process events
            match self.event_handler.next()? {
                Event::Key(key) => {
                    self.handle_key_event(key)?;
                    self.state.needs_redraw = true;
                }
                Event::Mouse(mouse) => {
                    self.handle_mouse_event(mouse)?;
                    self.state.needs_redraw = true;
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
                    // Save session on focus loss (with debounce)
                    if self.state.should_save_session() {
                        self.auto_save_session();
                        self.state.update_last_session_save();
                    }
                }
                Event::FocusGained => {
                    // Redraw on focus gain to refresh display
                    self.state.needs_redraw = true;
                }
                Event::Tick => {
                    // Check channel for directory size calculation results
                    self.check_dir_size_update();

                    // Check channel for git status update events
                    self.check_git_status_update();

                    // Check channel for filesystem update events
                    self.check_fs_update();

                    // Check pending git diff updates (debounced)
                    self.check_pending_git_diff_updates();

                    // Update system resource monitoring (CPU, RAM)
                    self.update_system_resources();

                    // Update spinner in Info modal if it's open
                    self.update_info_modal_spinner();
                }
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
            // Calculate available width for panel groups
            let terminal_width = self.state.terminal.width;

            let _ = self.layout_manager.close_active_panel(terminal_width);
            self.auto_save_session();
        }

        Ok(())
    }

    /// Check channel for directory size calculation results
    fn check_dir_size_update(&mut self) {
        use crate::state::ActiveModal;
        use termide_panel_file_manager::FileManager;

        if let Some(rx) = &self.state.dir_size_receiver {
            // Try to receive result without blocking
            if let Ok(result) = rx.try_recv() {
                // Update Info modal if it's open
                if let Some(ActiveModal::Info(ref mut modal)) = self.state.active_modal {
                    let t = termide_i18n::t();
                    let formatted_size = FileManager::format_size_static(result.size);
                    modal.update_value(t.file_info_size(), formatted_size);
                    self.state.needs_redraw = true;
                }

                // Clear channel
                self.state.dir_size_receiver = None;
            }
        }
    }

    /// Check channel for git status update events
    fn check_git_status_update(&mut self) {
        use termide_core::PanelCommand;

        // Lazy registration: register Editor file repositories with GitWatcher
        // Uses handle_command with GetRepoRoot to avoid downcasting
        if let Some(watcher) = &mut self.state.git_watcher {
            for panel in self.layout_manager.iter_all_panels_mut() {
                // Use handle_command to get repo root without downcasting
                if let termide_core::CommandResult::RepoRoot(Some(repo_root)) =
                    panel.handle_command(PanelCommand::GetRepoRoot)
                {
                    if !watcher.is_watching(&repo_root) {
                        let _ = watcher.watch_repository(repo_root);
                    }
                }
            }
        }

        // Collect all pending updates first to avoid borrowing conflicts
        let mut updates = Vec::new();
        if let Some(rx) = &self.state.git_watcher_receiver {
            while let Ok(update) = rx.try_recv() {
                updates.push(update);
            }
        }

        // Process collected updates using handle_command
        // IMPORTANT: Deduplicate to avoid O(updates × editors) git commands.
        if !updates.is_empty() {
            // Collect unique repo paths as Vec for slice conversion
            let repo_paths: Vec<_> = updates
                .iter()
                .map(|u| u.repo_path.as_path())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            // Update each panel at most once using handle_command
            for panel in self.layout_manager.iter_all_panels_mut() {
                if panel
                    .handle_command(PanelCommand::OnGitUpdate {
                        repo_paths: &repo_paths,
                    })
                    .needs_redraw()
                {
                    self.state.needs_redraw = true;
                }
            }
        }
    }

    /// Check channel for filesystem update events
    fn check_fs_update(&mut self) {
        use termide_core::{CommandResult, PanelCommand};
        use termide_git::find_repo_root;

        // Lazy registration: register all panel directories with watcher using handle_command
        if let Some(watcher) = &mut self.state.fs_watcher {
            for panel in self.layout_manager.iter_all_panels_mut() {
                // Use GetFsWatchInfo to check watch state without downcasting
                if let CommandResult::FsWatchInfo {
                    watched_root,
                    current_path,
                    is_git_repo: _,
                } = panel.handle_command(PanelCommand::GetFsWatchInfo)
                {
                    // Skip if already watching
                    if watched_root.is_some() {
                        continue;
                    }

                    // Determine the new watched root (only called once per directory change)
                    let repo_root = find_repo_root(&current_path);
                    let is_git_repo = repo_root.is_some();
                    let new_root = repo_root.unwrap_or_else(|| current_path.clone());

                    // Watch new root
                    if is_git_repo {
                        if !watcher.is_watching_repo(&new_root) {
                            let _ = watcher.watch_repository(new_root.clone());
                        }
                    } else if !watcher.is_watching_dir(&new_root) {
                        let _ = watcher.watch_directory(new_root.clone());
                    }

                    // Update panel's watched root
                    panel.handle_command(PanelCommand::SetFsWatchRoot {
                        root: Some(new_root),
                        is_git_repo,
                    });
                }

                // Also handle Editor panels via GetRepoRoot
                if let CommandResult::RepoRoot(Some(repo_root)) =
                    panel.handle_command(PanelCommand::GetRepoRoot)
                {
                    if !watcher.is_watching_repo(&repo_root) {
                        let _ = watcher.watch_repository(repo_root);
                    }
                }
            }
        }

        // Collect all pending updates first to avoid borrowing conflicts
        let mut updates = Vec::new();
        if let Some(rx) = &self.state.fs_watcher_receiver {
            while let Ok(update) = rx.try_recv() {
                updates.push(update);
            }
        }

        // Process collected updates using handle_command
        for update in updates {
            for panel in self.layout_manager.iter_all_panels_mut() {
                // Use OnFsUpdate command - panel decides if it needs to update
                if panel
                    .handle_command(PanelCommand::OnFsUpdate {
                        changed_path: &update.changed_path,
                    })
                    .needs_redraw()
                {
                    self.state.needs_redraw = true;
                }
            }
        }
    }

    /// Check and apply pending git diff updates (debounced) and async git diff results
    fn check_pending_git_diff_updates(&mut self) {
        use termide_core::PanelCommand;

        // Check all panels for pending git diff updates and async results using handle_command
        for panel in self.layout_manager.iter_all_panels_mut() {
            // Check debounced buffer updates
            panel.handle_command(PanelCommand::CheckPendingGitDiff);
            // Check async git diff results (from background thread)
            if panel
                .handle_command(PanelCommand::CheckGitDiffReceiver)
                .needs_redraw()
            {
                self.state.needs_redraw = true;
            }
        }
    }

    /// Update system resource monitoring (CPU, RAM)
    /// Respects the configured update interval
    fn update_system_resources(&mut self) {
        let interval =
            std::time::Duration::from_millis(self.state.config.logging.resource_monitor_interval);
        let elapsed = self.state.last_resource_update.elapsed();

        if elapsed >= interval {
            self.state.system_monitor.update();
            self.state.last_resource_update = std::time::Instant::now();
            self.state.needs_redraw = true;
        }
    }

    /// Update spinner in Info modal if it's open
    /// Throttled to 125ms (8 FPS) to reduce unnecessary redraws
    fn update_info_modal_spinner(&mut self) {
        use crate::state::ActiveModal;

        const SPINNER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(125);

        if let Some(ActiveModal::Info(ref mut modal)) = self.state.active_modal {
            // Update spinner only if calculation is still ongoing
            if self.state.dir_size_receiver.is_some() {
                // Throttle spinner updates
                let should_update = self
                    .state
                    .last_spinner_update
                    .is_none_or(|t| t.elapsed() >= SPINNER_INTERVAL);

                if should_update {
                    modal.advance_spinner();
                    self.state.last_spinner_update = Some(std::time::Instant::now());
                    self.state.needs_redraw = true;
                }
            }
        }
    }

    /// Save current session to file
    fn save_session(&mut self) -> Result<()> {
        // Get session directory for this project
        let session_dir = termide_session::Session::get_session_dir(&self.project_root)?;

        // Serialize layout to session (may save temporary buffers)
        let session = self.layout_manager.to_session(&session_dir);

        // Save session to file
        session.save(&self.project_root)?;
        termide_logger::info("Session saved");
        Ok(())
    }

    /// Load session from file and restore layout
    pub fn load_session(&mut self) -> Result<()> {
        // Load session for this project
        let session = termide_session::Session::load(&self.project_root)?;

        // Get session directory for restoring temporary buffers
        let session_dir = termide_session::Session::get_session_dir(&self.project_root)?;

        // Get terminal dimensions for creating Terminal panels
        let term_height = self.state.terminal.height.saturating_sub(3);
        let term_width = self.state.terminal.width.saturating_sub(2);

        // Restore layout from session
        self.layout_manager = LayoutManager::from_session(
            session,
            &session_dir,
            term_height,
            term_width,
            self.state.editor_config(),
        )?;
        termide_logger::info("Session loaded");

        // Clean up orphaned buffer files (not referenced in session anymore)
        if let Err(e) = termide_session::cleanup_orphaned_buffers(&session_dir) {
            termide_logger::warn(format!("Failed to cleanup orphaned buffers: {}", e));
        }

        Ok(())
    }

    /// Auto-save session (ignores errors to not disrupt user experience)
    pub fn auto_save_session(&mut self) {
        if let Err(e) = self.save_session() {
            // Log error but don't interrupt user workflow
            termide_logger::error(format!("Failed to auto-save session: {}", e));
        }
    }

    // ===== Panel downcast helpers =====
    // These helper methods reduce boilerplate for accessing specific panel types

    /// Get mutable reference to active editor panel
    /// Helper to avoid nested if-let chains: `if let Some(panel) = ... { if let Some(editor) = ... }`
    fn active_editor_mut(&mut self) -> Option<&mut termide_panel_editor::Editor> {
        self.layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.as_editor_mut())
    }

    /// Get mutable reference to active file manager panel
    /// Helper to avoid nested if-let chains: `if let Some(panel) = ... { if let Some(fm) = ... }`
    #[allow(dead_code)]
    fn active_file_manager_mut(&mut self) -> Option<&mut termide_panel_file_manager::FileManager> {
        self.layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.as_file_manager_mut())
    }

    /// Get reference to AppState
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Get mutable reference to AppState
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    /// Get reference to LayoutManager
    pub fn layout_manager(&self) -> &LayoutManager {
        &self.layout_manager
    }

    /// Get mutable reference to LayoutManager
    pub fn layout_manager_mut(&mut self) -> &mut LayoutManager {
        &mut self.layout_manager
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

impl LayoutController for App {
    fn add_panel(&mut self, panel: Box<dyn Panel>) {
        let terminal_width = self.state.terminal.width;
        let config = &self.state.config;
        self.layout_manager.add_panel(panel, config, terminal_width);
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
