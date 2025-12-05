use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};
use std::any::Any;

use crate::state::AppState;

pub mod debug;
pub mod editor;
pub mod file_manager;
pub mod panel_ext;
pub mod panel_group;
pub mod terminal;
pub mod terminal_pty;
pub mod welcome;

pub use panel_ext::PanelExt;

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

    /// Serialize panel to session data
    /// Returns None for panels that shouldn't be saved (e.g., Welcome)
    ///
    /// # Parameters
    /// - `session_dir`: Directory where session files are stored (for saving temporary buffers)
    fn to_session_panel(
        &mut self,
        session_dir: &std::path::Path,
    ) -> Option<crate::session::SessionPanel> {
        let _ = session_dir; // Unused in default implementation
        None
    }
}
