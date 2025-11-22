use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};
use std::any::Any;

use crate::state::AppState;

pub mod debug;
pub mod editor;
pub mod file_manager;
pub mod terminal_pty;
pub mod welcome;

/// Trait for all application panels
pub trait Panel: Any {
    /// Render the panel
    /// panel_index - panel index in container (for displaying [X] button)
    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        is_focused: bool,
        panel_index: usize,
        state: &AppState,
    );

    /// Handle keyboard event
    fn handle_key(&mut self, key: KeyEvent) -> Result<()>;

    /// Handle mouse event
    /// Returns (relative coordinates inside the panel)
    fn handle_mouse(
        &mut self,
        _mouse: crossterm::event::MouseEvent,
        _panel_area: Rect,
    ) -> Result<()> {
        Ok(())
    }

    /// Get panel title (can be dynamic)
    fn title(&self) -> String;

    /// Check if panel should automatically close
    /// Returns true if panel should be closed (e.g., terminal after process completion)
    fn should_auto_close(&self) -> bool {
        false
    }

    /// Get file to open in editor (for FileManager)
    /// Returns Some(PathBuf) if user selected a file to open
    fn take_file_to_open(&mut self) -> Option<std::path::PathBuf> {
        None
    }

    /// Get current working directory (for FM and terminals)
    fn get_working_directory(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// Take modal window request (if any)
    /// Returns (PendingAction, ActiveModal) if panel requests a modal window
    fn take_modal_request(
        &mut self,
    ) -> Option<(crate::state::PendingAction, crate::state::ActiveModal)> {
        None
    }

    /// Check if confirmation is required before closing panel
    /// Returns Some(message) if confirmation needed, None if can close without confirmation
    fn needs_close_confirmation(&self) -> Option<String> {
        None
    }

    /// Check if panel should capture Escape
    /// If true, Escape doesn't close the panel but is passed to handle_key
    fn captures_escape(&self) -> bool {
        false
    }

    /// Reload panel content
    /// Called when another panel closes or when state needs to be updated
    fn reload(&mut self) -> Result<()> {
        Ok(())
    }

    /// Check if there are running child processes (for terminal)
    fn has_running_processes(&self) -> bool {
        false
    }

    /// Terminate all child processes (for terminal)
    fn kill_processes(&mut self) {}

    /// Check if this is a Welcome panel
    /// Welcome panel automatically closes when other panels are opened
    fn is_welcome_panel(&self) -> bool {
        false
    }
}

/// Container for panels
pub struct PanelContainer {
    panels: Vec<Box<dyn Panel>>,
    visible: Vec<bool>,
}

impl PanelContainer {
    /// Create new panel container
    pub fn new() -> Self {
        Self {
            panels: Vec::new(),
            visible: Vec::new(),
        }
    }

    /// Add panel
    pub fn add_panel(&mut self, panel: Box<dyn Panel>) {
        self.panels.push(panel);
        self.visible.push(true); // Panel is visible by default
    }

    /// Get number of panels
    pub fn count(&self) -> usize {
        self.panels.len()
    }

    /// Get panel by index
    pub fn get(&self, index: usize) -> Option<&Box<dyn Panel>> {
        self.panels.get(index)
    }

    /// Get panel by index (mutable)
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Box<dyn Panel>> {
        self.panels.get_mut(index)
    }

    /// Close panel (remove from list)
    pub fn close_panel(&mut self, index: usize) {
        if index < self.panels.len() {
            self.panels.remove(index);
            self.visible.remove(index);
        }
    }

    /// Swap two panels
    pub fn swap_panels(&mut self, index1: usize, index2: usize) {
        if index1 < self.panels.len() && index2 < self.panels.len() {
            self.panels.swap(index1, index2);
            self.visible.swap(index1, index2);
        }
    }

    /// Get indices of all visible panels
    pub fn visible_indices(&self) -> Vec<usize> {
        self.visible
            .iter()
            .enumerate()
            .filter_map(|(i, &visible)| if visible { Some(i) } else { None })
            .collect()
    }

    /// Check if there are visible panels
    pub fn has_visible_panels(&self) -> bool {
        self.visible.iter().any(|&v| v)
    }
}

impl Default for PanelContainer {
    fn default() -> Self {
        Self::new()
    }
}
