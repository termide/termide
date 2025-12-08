use anyhow::Result;
use ratatui::{backend::Backend, Terminal};
use std::time::Duration;

use crate::{
    event::{Event, EventHandler},
    layout_manager::LayoutManager,
    panels::PanelExt,
    state::{ActiveModal, AppState},
    ui::render_layout_with_accordion,
};

mod key_handler;
mod modal;
mod modal_handler;
mod mouse_handler;
mod panel_manager;

/// Main application
pub struct App {
    state: AppState,
    layout_manager: LayoutManager,
    event_handler: EventHandler,
    /// Project root directory (used for per-project session storage)
    project_root: std::path::PathBuf,
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
        let log_file_path = if let Some(ref path) = state.config.log_file_path {
            std::path::PathBuf::from(path)
        } else {
            crate::session::Session::get_session_dir(&project_root)
                .map(|dir| {
                    // Cleanup old log files (older than 24 hours)
                    let _ = crate::session::cleanup_old_logs(&dir);
                    dir.join(crate::session::generate_log_filename())
                })
                .unwrap_or_else(|_| {
                    std::env::temp_dir().join(crate::session::generate_log_filename())
                })
        };
        let min_log_level = crate::logger::LogLevel::from_str(&state.config.min_log_level)
            .unwrap_or(crate::logger::LogLevel::Info);
        crate::logger::init(
            log_file_path,
            crate::constants::MAX_LOG_ENTRIES,
            min_log_level,
        );
        crate::logger::info("Application started");

        // Initialize git watcher for automatic status updates
        match crate::git::create_git_watcher() {
            Ok((watcher, receiver)) => {
                state.git_watcher = Some(watcher);
                state.git_watcher_receiver = Some(receiver);
                crate::logger::info("Git watcher initialized");
            }
            Err(e) => {
                crate::logger::error(format!("Failed to initialize git watcher: {}", e));
            }
        }

        // Initialize filesystem watcher for automatic directory updates
        match crate::fs_watcher::create_fs_watcher() {
            Ok((watcher, receiver)) => {
                state.fs_watcher = Some(watcher);
                state.fs_watcher_receiver = Some(receiver);
                crate::logger::info("FS watcher initialized");
            }
            Err(e) => {
                crate::logger::error(format!("Failed to initialize FS watcher: {}", e));
            }
        }

        // Clean up old sessions (configurable retention period)
        let retention_days = state.config.session_retention_days;
        if let Err(e) = crate::session::cleanup_old_sessions(&project_root, retention_days) {
            crate::logger::warn(format!("Failed to cleanup old sessions: {}", e));
        }

        Self {
            state,
            layout_manager: LayoutManager::new(),
            event_handler: EventHandler::new(Duration::from_millis(
                crate::constants::EVENT_HANDLER_INTERVAL_MS,
            )),
            project_root,
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
    pub fn add_panel(&mut self, panel: Box<dyn crate::panels::Panel>) {
        let terminal_width = self.state.terminal.width;
        let config = &self.state.config;
        self.layout_manager.add_panel(panel, config, terminal_width);
    }

    /// Run the main application loop
    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()> {
        // Initialize terminal dimensions
        let size = terminal.size()?;
        self.state.update_terminal_size(size.width, size.height);

        while !self.state.should_quit {
            // Process events
            match self.event_handler.next()? {
                Event::Key(key) => {
                    self.handle_key_event(key)?;
                }
                Event::Mouse(mouse) => {
                    self.handle_mouse_event(mouse)?;
                }
                Event::Resize(width, height) => {
                    // Update terminal dimensions in state
                    self.state.update_terminal_size(width, height);

                    // Пропорционально перераспределить ширины групп при изменении размера терминала
                    self.layout_manager
                        .redistribute_widths_proportionally(width);
                }
                Event::FocusLost => {
                    // Save session on focus loss (with debounce)
                    if self.state.should_save_session() {
                        self.auto_save_session();
                        self.state.update_last_session_save();
                    }
                }
                Event::FocusGained => {
                    // Currently no action needed on focus gain
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

            // Render UI after processing event
            terminal.draw(|frame| {
                render_layout_with_accordion(frame, &mut self.state, &mut self.layout_manager);
            })?;
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
        use crate::panels::file_manager::FileManager;

        if let Some(rx) = &self.state.dir_size_receiver {
            // Try to receive result without blocking
            if let Ok(result) = rx.try_recv() {
                // Update Info modal if it's open
                if let Some(ActiveModal::Info(ref mut modal)) = self.state.active_modal {
                    let t = crate::i18n::t();
                    let formatted_size = FileManager::format_size_static(result.size);
                    modal.update_value(t.file_info_size(), formatted_size);
                }

                // Clear channel
                self.state.dir_size_receiver = None;
            }
        }
    }

    /// Check channel for git status update events
    fn check_git_status_update(&mut self) {
        use crate::panels::file_manager::FileManager;

        // First, register all FileManager repositories with watcher (lazy registration)
        if let Some(watcher) = &mut self.state.git_watcher {
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(fm) =
                    (&mut **panel as &mut dyn std::any::Any).downcast_mut::<FileManager>()
                {
                    // Find repository root for this FileManager's current path
                    if let Some(repo_root) = Self::find_git_repo_root(fm.current_path()) {
                        // Only register if not already watching
                        if !watcher.is_watching(&repo_root) {
                            let _ = watcher.watch_repository(repo_root);
                        }
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

        // Process collected updates
        for update in updates {
            // Update all FileManager panels showing this repository or its subdirectories
            for panel in self.layout_manager.iter_all_panels_mut() {
                // Try to downcast to FileManager
                if let Some(fm) =
                    (&mut **panel as &mut dyn std::any::Any).downcast_mut::<FileManager>()
                {
                    // Check if this panel is showing a path within the updated repository
                    if fm.current_path().starts_with(&update.repo_path) {
                        let _ = fm.update_git_status();
                    }
                }

                // Try to downcast to Editor
                use crate::panels::editor::Editor;
                if let Some(editor) =
                    (&mut **panel as &mut dyn std::any::Any).downcast_mut::<Editor>()
                {
                    // Check if this editor has a file in the updated repository
                    if let Some(file_path) = editor.file_path() {
                        if file_path.starts_with(&update.repo_path) {
                            editor.update_git_diff();
                        }
                    }
                }
            }
        }
    }

    /// Find git repository root by walking up from a path
    fn find_git_repo_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
        let mut current = path;
        loop {
            if current.join(".git").exists() {
                return Some(current.to_path_buf());
            }
            current = current.parent()?;
        }
    }

    /// Check channel for filesystem update events
    fn check_fs_update(&mut self) {
        use crate::git::find_repo_root;
        use crate::panels::file_manager::FileManager;

        // Lazy registration: register all FileManager directories with watcher
        // Also handle watch/unwatch when navigating between repositories
        if let Some(watcher) = &mut self.state.fs_watcher {
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(fm) =
                    (&mut **panel as &mut dyn std::any::Any).downcast_mut::<FileManager>()
                {
                    let current_path = fm.current_path();

                    // Determine the new watched root
                    let new_root =
                        find_repo_root(current_path).unwrap_or_else(|| current_path.to_path_buf());

                    // Check if watched root changed (navigation between repos/dirs)
                    let root_changed = fm.watched_root().map(|r| r != &new_root).unwrap_or(true);

                    if root_changed {
                        // Unwatch old root if exists
                        if let Some(old_root) = fm.watched_root() {
                            if find_repo_root(old_root).is_some() {
                                watcher.unwatch_repository(old_root);
                            } else {
                                watcher.unwatch_directory(old_root);
                            }
                        }

                        // Watch new root
                        if find_repo_root(current_path).is_some() {
                            // Git repository: watch recursively from repo root
                            if !watcher.is_watching_repo(&new_root) {
                                let _ = watcher.watch_repository(new_root.clone());
                            }
                        } else {
                            // Non-git directory: watch non-recursively
                            if !watcher.is_watching_dir(&new_root) {
                                let _ = watcher.watch_directory(new_root.clone());
                            }
                        }

                        fm.set_watched_root(Some(new_root));
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

        // Process collected updates
        for update in updates {
            // Update all FileManager panels showing this directory
            for panel in self.layout_manager.iter_all_panels_mut() {
                // Try to downcast to FileManager
                if let Some(fm) =
                    (&mut **panel as &mut dyn std::any::Any).downcast_mut::<FileManager>()
                {
                    let current = fm.current_path();
                    let changed_parent = update.changed_path.parent();

                    // Reload if:
                    // 1. The changed path's parent is our current directory (file in current dir changed)
                    // 2. The changed path IS our current directory (directory itself renamed/deleted)
                    let should_reload =
                        changed_parent == Some(current) || update.changed_path == current;

                    if should_reload {
                        let _ = fm.load_directory();
                    }
                }

                // Try to downcast to Editor
                use crate::panels::editor::Editor;
                if let Some(editor) =
                    (&mut **panel as &mut dyn std::any::Any).downcast_mut::<Editor>()
                {
                    // Check if this editor's file was changed
                    if let Some(file_path) = editor.file_path() {
                        // Check for exact file match or directory containing the file
                        let file_changed = update.changed_path == file_path
                            || update.changed_path.parent() == file_path.parent();

                        if file_changed {
                            // File might have changed - update git diff
                            editor.update_git_diff();
                            // Check for external modification
                            editor.check_external_modification();
                        }
                    }
                }
            }
        }
    }

    /// Check and apply pending git diff updates (debounced)
    fn check_pending_git_diff_updates(&mut self) {
        use crate::panels::editor::Editor;

        // Check all Editor panels for pending git diff updates
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(editor) = (&mut **panel as &mut dyn std::any::Any).downcast_mut::<Editor>()
            {
                editor.check_pending_git_diff_update();
            }
        }
    }

    /// Update system resource monitoring (CPU, RAM)
    /// Respects the configured update interval
    fn update_system_resources(&mut self) {
        let interval =
            std::time::Duration::from_millis(self.state.config.resource_monitor_interval);
        let elapsed = self.state.last_resource_update.elapsed();

        if elapsed >= interval {
            self.state.system_monitor.update();
            self.state.last_resource_update = std::time::Instant::now();
        }
    }

    /// Update spinner in Info modal if it's open
    fn update_info_modal_spinner(&mut self) {
        if let Some(ActiveModal::Info(ref mut modal)) = self.state.active_modal {
            // Update spinner only if calculation is still ongoing
            if self.state.dir_size_receiver.is_some() {
                modal.advance_spinner();
            }
        }
    }

    /// Save current session to file
    fn save_session(&mut self) -> Result<()> {
        // Get session directory for this project
        let session_dir = crate::session::Session::get_session_dir(&self.project_root)?;

        // Serialize layout to session (may save temporary buffers)
        let session = self.layout_manager.to_session(&session_dir);

        // Save session to file
        session.save(&self.project_root)?;
        crate::logger::info("Session saved");
        Ok(())
    }

    /// Load session from file and restore layout
    pub fn load_session(&mut self) -> Result<()> {
        // Load session for this project
        let session = crate::session::Session::load(&self.project_root)?;

        // Get session directory for restoring temporary buffers
        let session_dir = crate::session::Session::get_session_dir(&self.project_root)?;

        // Get terminal dimensions for creating Terminal panels
        let term_height = self.state.terminal.height.saturating_sub(3);
        let term_width = self.state.terminal.width.saturating_sub(2);

        // Restore layout from session
        self.layout_manager = crate::layout_manager::LayoutManager::from_session(
            session,
            &session_dir,
            term_height,
            term_width,
            self.state.editor_config(),
        )?;
        crate::logger::info("Session loaded");

        // Clean up orphaned buffer files (not referenced in session anymore)
        if let Err(e) = crate::session::cleanup_orphaned_buffers(&session_dir) {
            crate::logger::warn(format!("Failed to cleanup orphaned buffers: {}", e));
        }

        Ok(())
    }

    /// Auto-save session (ignores errors to not disrupt user experience)
    pub fn auto_save_session(&mut self) {
        if let Err(e) = self.save_session() {
            // Log error but don't interrupt user workflow
            crate::logger::error(format!("Failed to auto-save session: {}", e));
        }
    }

    // ===== Panel downcast helpers =====
    // These helper methods reduce boilerplate for accessing specific panel types

    /// Get mutable reference to active editor panel
    /// Helper to avoid nested if-let chains: `if let Some(panel) = ... { if let Some(editor) = ... }`
    fn active_editor_mut(&mut self) -> Option<&mut crate::panels::editor::Editor> {
        self.layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.as_editor_mut())
    }

    /// Get mutable reference to active file manager panel
    /// Helper to avoid nested if-let chains: `if let Some(panel) = ... { if let Some(fm) = ... }`
    #[allow(dead_code)]
    fn active_file_manager_mut(&mut self) -> Option<&mut crate::panels::file_manager::FileManager> {
        self.layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.as_file_manager_mut())
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
