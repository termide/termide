//! Panel trait definition for termide panels.
//!
//! The new Panel trait is designed to be decoupled from AppState,
//! using event-driven communication instead.

use std::any::Any;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use termide_config::Config;
use termide_theme::Theme;

use crate::{CommandResult, PanelCommand, PanelEvent};

// Re-export SessionPanel from termide-session for unified type
pub use termide_session::SessionPanel;

/// Configuration settings relevant to panels.
///
/// Subset of the full application config that panels need for rendering.
#[derive(Debug, Clone)]
pub struct PanelConfig {
    /// Tab size for editor
    pub tab_size: usize,
    /// Enable word wrapping
    pub word_wrap: bool,
    /// Show line numbers in editor
    pub show_line_numbers: bool,
    /// Show hidden files in file manager
    pub show_hidden_files: bool,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            tab_size: 4,
            word_wrap: false,
            show_line_numbers: true,
            show_hidden_files: false,
        }
    }
}

/// Render context passed to panels during rendering.
///
/// Contains all information a panel needs for rendering
/// without requiring access to the full application state.
pub struct RenderContext<'a> {
    /// Current theme colors
    pub theme: &'a ThemeColors,
    /// Panel configuration
    pub config: &'a PanelConfig,
    /// Whether this panel is currently focused
    pub is_focused: bool,
    /// Panel index in container (for displaying [X] button)
    pub panel_index: usize,
    /// Terminal width
    pub terminal_width: u16,
    /// Terminal height
    pub terminal_height: u16,
}

/// Minimal theme colors needed for rendering.
///
/// This is a subset of the full Theme, containing only
/// the colors needed for panel rendering.
#[derive(Debug, Clone)]
pub struct ThemeColors {
    pub fg: Color,
    pub bg: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub border: Color,
    pub border_focused: Color,
    pub line_numbers: Color,
    pub cursor: Color,
    pub status_bar_bg: Color,
    pub status_bar_fg: Color,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            fg: Color::White,
            bg: Color::Black,
            selection_bg: Color::Blue,
            selection_fg: Color::White,
            border: Color::DarkGray,
            border_focused: Color::Cyan,
            line_numbers: Color::DarkGray,
            cursor: Color::Yellow,
            status_bar_bg: Color::DarkGray,
            status_bar_fg: Color::White,
        }
    }
}

impl From<&Theme> for ThemeColors {
    fn from(theme: &Theme) -> Self {
        Self {
            fg: theme.fg,
            bg: theme.bg,
            selection_bg: theme.selected_bg,
            selection_fg: theme.selected_fg,
            border: theme.disabled,
            border_focused: theme.accented_fg,
            line_numbers: theme.disabled,
            cursor: theme.accented_fg,
            status_bar_bg: theme.accented_bg,
            status_bar_fg: theme.fg,
        }
    }
}

/// Trait for all termide panels.
///
/// Panels communicate with the application through `PanelEvent`s
/// instead of directly modifying application state.
pub trait Panel: Any {
    /// Unique name for panel identification.
    fn name(&self) -> &'static str;

    /// Dynamic title for display in the panel header.
    fn title(&self) -> String;

    /// Prepare panel for rendering (update cached theme/config).
    ///
    /// Called before render() to sync panel's internal state with current app state.
    fn prepare_render(&mut self, theme: &Theme, config: &Config) {
        let _ = (theme, config);
    }

    /// Render the panel to the buffer.
    ///
    /// # Arguments
    /// * `area` - The area to render into
    /// * `buf` - The buffer to render to
    /// * `ctx` - Render context with theme and focus info
    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext);

    /// Handle keyboard input.
    ///
    /// Returns a list of events to be processed by the application.
    fn handle_key(&mut self, key: KeyEvent) -> Vec<PanelEvent>;

    /// Handle mouse input.
    ///
    /// # Arguments
    /// * `event` - The mouse event
    /// * `panel_area` - The panel's area (for coordinate translation)
    fn handle_mouse(&mut self, event: MouseEvent, panel_area: Rect) -> Vec<PanelEvent> {
        let _ = (event, panel_area);
        vec![]
    }

    /// Periodic tick for background tasks.
    ///
    /// Called periodically to allow panels to perform background work
    /// and emit events.
    fn tick(&mut self) -> Vec<PanelEvent> {
        vec![]
    }

    /// Handle a command from the application.
    ///
    /// Commands allow the App to interact with panels without downcasting.
    /// Each panel type implements only the commands it supports.
    ///
    /// # Arguments
    /// * `cmd` - The command to handle
    ///
    /// # Returns
    /// A result indicating the outcome of the command.
    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        let _ = cmd;
        CommandResult::None
    }

    /// Check if panel should automatically close.
    ///
    /// Returns true if panel should be closed
    /// (e.g., terminal after process completion).
    fn should_auto_close(&self) -> bool {
        false
    }

    /// Check if panel needs confirmation before closing.
    ///
    /// Returns Some(message) if confirmation is needed (e.g., unsaved changes).
    fn needs_close_confirmation(&self) -> Option<String> {
        None
    }

    /// Check if panel captures Escape key.
    ///
    /// Returns true if panel handles Escape internally
    /// (e.g., when search mode is active).
    fn captures_escape(&self) -> bool {
        false
    }

    /// Reload panel content from source.
    ///
    /// Used when file is modified externally.
    fn reload(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Serialize panel state for session persistence.
    ///
    /// Returns None if panel should not be saved in session.
    /// The session_dir is provided for saving unsaved buffers.
    fn to_session(&self, session_dir: &Path) -> Option<SessionPanel> {
        let _ = session_dir;
        None
    }

    /// Downcast to concrete type (immutable).
    fn as_any(&self) -> &dyn Any;

    /// Downcast to concrete type (mutable).
    fn as_any_mut(&mut self) -> &mut dyn Any;

    // === Additional methods for application integration ===

    /// Get current working directory (for file manager and terminals).
    fn get_working_directory(&self) -> Option<PathBuf> {
        None
    }

    /// Check if there are running child processes (for terminal).
    fn has_running_processes(&self) -> bool {
        false
    }

    /// Terminate all child processes (for terminal).
    fn kill_processes(&mut self) {}

    /// Check if this is a Welcome panel.
    fn is_welcome_panel(&self) -> bool {
        false
    }
}
