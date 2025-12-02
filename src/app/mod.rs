use anyhow::Result;
use ratatui::{backend::Backend, Terminal};
use std::time::Duration;

use crate::{
    event::{Event, EventHandler},
    layout_manager::LayoutManager,
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
}

impl App {
    /// Create a new application
    pub fn new() -> Self {
        let mut state = AppState::new();

        // Initialize git watcher for automatic status updates
        match crate::git::create_git_watcher() {
            Ok((watcher, receiver)) => {
                state.git_watcher = Some(watcher);
                state.git_watcher_receiver = Some(receiver);
                state.log_info("Git watcher initialized");
            }
            Err(e) => {
                state.log_error(format!("Failed to initialize git watcher: {}", e));
            }
        }

        // Initialize filesystem watcher for automatic directory updates
        match crate::fs_watcher::create_fs_watcher() {
            Ok((watcher, receiver)) => {
                state.fs_watcher = Some(watcher);
                state.fs_watcher_receiver = Some(receiver);
                state.log_info("FS watcher initialized");
            }
            Err(e) => {
                state.log_error(format!("Failed to initialize FS watcher: {}", e));
            }
        }

        Self {
            state,
            layout_manager: LayoutManager::new(),
            event_handler: EventHandler::new(Duration::from_millis(
                crate::constants::EVENT_HANDLER_INTERVAL_MS,
            )),
        }
    }

    /// Add a panel (automatically stacks if width threshold is reached)
    pub fn add_panel(&mut self, panel: Box<dyn crate::panels::Panel>) {
        let terminal_width = self.state.terminal.width;
        let config = &self.state.config;
        self.layout_manager.add_panel(panel, config, terminal_width);
    }

    /// Set the file manager panel
    pub fn set_file_manager(&mut self, fm: Box<dyn crate::panels::Panel>) {
        self.layout_manager.set_file_manager(fm);
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
                    let fm_width = if self.layout_manager.has_file_manager() {
                        crate::constants::DEFAULT_FM_WIDTH
                    } else {
                        0
                    };
                    let available_width = width.saturating_sub(fm_width);
                    self.layout_manager
                        .redistribute_widths_proportionally(available_width);
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
            let fm_width = if self.layout_manager.has_file_manager() {
                crate::constants::DEFAULT_FM_WIDTH
            } else {
                0
            };
            let available_width = terminal_width.saturating_sub(fm_width);

            let _ = self.layout_manager.close_active_panel(available_width);
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
        use crate::panels::file_manager::FileManager;

        // Lazy registration: register all FileManager directories with watcher
        if let Some(watcher) = &mut self.state.fs_watcher {
            for panel in self.layout_manager.iter_all_panels_mut() {
                if let Some(fm) =
                    (&mut **panel as &mut dyn std::any::Any).downcast_mut::<FileManager>()
                {
                    let dir_path = fm.current_path().to_path_buf();

                    // Only register if not already watching
                    if !watcher.is_watching(&dir_path) {
                        let _ = watcher.watch_directory(dir_path);
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
                    // Check if this panel is showing the updated directory
                    if fm.current_path() == update.dir_path {
                        let _ = fm.load_directory();
                    }
                }

                // Try to downcast to Editor
                use crate::panels::editor::Editor;
                if let Some(editor) =
                    (&mut **panel as &mut dyn std::any::Any).downcast_mut::<Editor>()
                {
                    // Check if this editor has a file in the updated directory
                    if let Some(file_path) = editor.file_path() {
                        if file_path.parent() == Some(&update.dir_path) {
                            // File in this directory might have changed - update git diff
                            editor.update_git_diff();
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
    fn save_session(&self) -> Result<()> {
        let session = self.layout_manager.to_session();
        session.save()?;
        Ok(())
    }

    /// Load session from file and restore layout
    pub fn load_session(&mut self) -> Result<()> {
        let session = crate::session::Session::load()?;

        // Get terminal dimensions for creating Terminal panels
        let term_height = self.state.terminal.height.saturating_sub(3);
        let term_width = self.state.terminal.width.saturating_sub(2);

        // Restore layout from session
        self.layout_manager =
            crate::layout_manager::LayoutManager::from_session(session, term_height, term_width)?;

        Ok(())
    }

    /// Auto-save session (ignores errors to not disrupt user experience)
    pub fn auto_save_session(&self) {
        if let Err(e) = self.save_session() {
            // Log error but don't interrupt user workflow
            // In a production app, you might want to log this to a file
            eprintln!("Warning: Failed to auto-save session: {}", e);
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
