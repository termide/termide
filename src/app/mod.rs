use anyhow::Result;
use ratatui::{backend::Backend, Terminal};
use std::time::Duration;

use crate::{
    event::{Event, EventHandler},
    panels::PanelContainer,
    state::{ActiveModal, AppState},
    ui::render_layout,
};

mod key_handler;
mod modal;
mod modal_handler;
mod mouse_handler;
mod panel_manager;

/// Main application
pub struct App {
    state: AppState,
    panels: PanelContainer,
    event_handler: EventHandler,
}

impl App {
    /// Create a new application
    pub fn new() -> Self {
        let mut state = AppState::new();
        // Set active panel to FileManager (panel 0) by default
        state.active_panel = 0;

        // Initialize git watcher for automatic status updates
        match crate::git::create_git_watcher() {
            Ok((watcher, receiver)) => {
                state.git_watcher = Some(watcher);
                state.git_watcher_receiver = Some(receiver);
                state.log_info("Git watcher initialized");
            }
            Err(e) => {
                state.log_error(&format!("Failed to initialize git watcher: {}", e));
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
                state.log_error(&format!("Failed to initialize FS watcher: {}", e));
            }
        }

        Self {
            state,
            panels: PanelContainer::new(),
            event_handler: EventHandler::new(Duration::from_millis(crate::constants::EVENT_HANDLER_INTERVAL_MS)),
        }
    }

    /// Add a panel
    pub fn add_panel(&mut self, panel: Box<dyn crate::panels::Panel>) {
        self.panels.add_panel(panel);
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
                }
                Event::Tick => {
                    // Check channel for directory size calculation results
                    self.check_dir_size_update();

                    // Check channel for git status update events
                    self.check_git_status_update();

                    // Check channel for filesystem update events
                    self.check_fs_update();

                    // Update spinner in Info modal if it's open
                    self.update_info_modal_spinner();
                }
            }

            // Check and close panels that should auto-close
            self.check_auto_close_panels()?;

            // Render UI after processing event
            terminal.draw(|frame| {
                render_layout(frame, &self.state, &mut self.panels);
            })?;
        }

        Ok(())
    }

    /// Check and close panels that should auto-close
    fn check_auto_close_panels(&mut self) -> Result<()> {
        // Collect panel indices to close
        let mut to_close = Vec::new();

        for i in 0..self.panels.count() {
            if let Some(panel) = self.panels.get_mut(i) {
                if panel.should_auto_close() {
                    // Check if this panel can be closed
                    if crate::ui::panel_helpers::can_close_panel(i, &self.state) {
                        to_close.push(i);
                    }
                }
            }
        }

        // Close panels (in reverse order so indices don't shift)
        for &index in to_close.iter().rev() {
            self.panels.close_panel(index);

            // If closed active panel, switch to next
            if index == self.state.active_panel {
                if self.panels.count() > 0 {
                    let visible = self.panels.visible_indices();
                    if !visible.is_empty() {
                        self.state.active_panel = visible
                            .iter()
                            .find(|&&i| i >= index)
                            .or_else(|| visible.last())
                            .copied()
                            .unwrap_or(0);
                    }
                }
            } else if index < self.state.active_panel && self.state.active_panel > 0 {
                // If closed panel to the left of active, shift index
                self.state.active_panel -= 1;
            }
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

        // Collect repositories to register
        let mut repos_to_register = Vec::new();

        // First, register all FileManager repositories with watcher (lazy registration)
        if let Some(watcher) = &mut self.state.git_watcher {
            for i in 0..self.panels.count() {
                if let Some(panel) = self.panels.get_mut(i) {
                    if let Some(fm) = (panel as &mut dyn std::any::Any).downcast_mut::<FileManager>() {
                        // Find repository root for this FileManager's current path
                        if let Some(repo_root) = Self::find_git_repo_root(fm.current_path()) {
                            // Only register if not already watching (avoid log spam)
                            if !watcher.is_watching(&repo_root) {
                                if let Ok(()) = watcher.watch_repository(repo_root.clone()) {
                                    repos_to_register.push(repo_root);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Log registered repositories after releasing watcher borrow
        for repo_root in repos_to_register {
            self.state.log_info(&format!("Git watcher: registered {}", repo_root.display()));
        }

        let mut updated_count = 0;

        // Collect all pending updates first to avoid borrowing conflicts
        let mut updates = Vec::new();
        if let Some(rx) = &self.state.git_watcher_receiver {
            while let Ok(update) = rx.try_recv() {
                updates.push(update);
            }
        }

        // Process collected updates
        for update in updates {
            self.state.log_info(&format!("[GitWatcher] Received update for repo: {:?}", update.repo_path));

            // Update all FileManager panels showing this repository or its subdirectories
            for i in 0..self.panels.count() {
                if let Some(panel) = self.panels.get_mut(i) {
                    // Try to downcast to FileManager
                    if let Some(fm) = (panel as &mut dyn std::any::Any).downcast_mut::<FileManager>() {
                        // Check if this panel is showing a path within the updated repository
                        if fm.current_path().starts_with(&update.repo_path) {
                            let _ = fm.update_git_status();
                            updated_count += 1;
                        }
                    }
                }
            }
        }

        if updated_count > 0 {
            self.state.log_info(&format!("Git status updated for {} panels", updated_count));
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

        // Collect directories to register
        let mut dirs_to_register = Vec::new();

        // Lazy registration: register all FileManager directories with watcher
        if let Some(watcher) = &mut self.state.fs_watcher {
            for i in 0..self.panels.count() {
                if let Some(panel) = self.panels.get_mut(i) {
                    if let Some(fm) = (panel as &mut dyn std::any::Any).downcast_mut::<FileManager>() {
                        let dir_path = fm.current_path().to_path_buf();

                        // Only register if not already watching
                        if !watcher.is_watching(&dir_path) {
                            if let Ok(()) = watcher.watch_directory(dir_path.clone()) {
                                dirs_to_register.push(dir_path);
                            }
                        }
                    }
                }
            }
        }

        // Log registered directories after releasing watcher borrow
        for dir_path in dirs_to_register {
            self.state.log_info(&format!("FS watcher: registered {}", dir_path.display()));
        }

        let mut updated_count = 0;

        // Collect all pending updates first to avoid borrowing conflicts
        let mut updates = Vec::new();
        if let Some(rx) = &self.state.fs_watcher_receiver {
            while let Ok(update) = rx.try_recv() {
                updates.push(update);
            }
        }

        // Process collected updates
        for update in updates {
            self.state.log_info(&format!("[FSWatcher] Received update for dir: {:?}", update.dir_path));

            // Update all FileManager panels showing this directory
            for i in 0..self.panels.count() {
                if let Some(panel) = self.panels.get_mut(i) {
                    // Try to downcast to FileManager
                    if let Some(fm) = (panel as &mut dyn std::any::Any).downcast_mut::<FileManager>() {
                        // Check if this panel is showing the updated directory
                        if fm.current_path() == &update.dir_path {
                            let _ = fm.load_directory();
                            updated_count += 1;
                        }
                    }
                }
            }
        }

        if updated_count > 0 {
            self.state.log_info(&format!("FS: reloaded {} panels", updated_count));
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
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
