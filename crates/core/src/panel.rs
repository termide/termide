//! Panel trait definition for termide panels.
//!
//! The new Panel trait is designed to be decoupled from AppState,
//! using event-driven communication instead.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::MouseEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};
use termide_config::Config;
use termide_theme::Theme;

use crate::{CommandResult, KeyChord, PanelCommand, PanelEvent};

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
    /// X position of right border (for scrollbar rendering on border)
    pub border_right_x: Option<u16>,
    /// Y position of the bottom border (for a horizontal scrollbar drawn on the
    /// frame). `None` when the panel has no bottom border (e.g. side-by-side
    /// layouts that omit it).
    pub border_bottom_y: Option<u16>,
}

/// Minimal theme colors needed for rendering.
///
/// This is a subset of the full Theme, containing only
/// the colors needed for panel rendering.
#[derive(Debug, Clone, Copy)]
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
    // Semantic colors
    pub disabled: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
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
            disabled: Color::DarkGray,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            info: Color::Cyan,
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
            disabled: theme.disabled,
            success: theme.success,
            warning: theme.warning,
            error: theme.error,
            info: theme.accented_fg, // Use accented_fg as info color
        }
    }
}

impl ThemeColors {
    /// Determine if this is a light theme based on background luminance.
    /// Uses ITU-R BT.601 relative luminance formula.
    pub fn is_light_theme(&self) -> bool {
        match self.bg {
            Color::Rgb(r, g, b) => {
                let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                luminance > 128.0
            }
            Color::White | Color::Gray => true,
            _ => false,
        }
    }
}

/// Unified search interface for panels that support text search.
///
/// Implemented by Editor (text search) and Terminal (scrollback search).
/// This trait allows the modal handler to dispatch search actions
/// without knowing the concrete panel type.
pub trait Searchable {
    /// Start a new search with the given query. `use_regex` treats the query
    /// as a regular expression (panels that don't support regex may ignore it
    /// and match literally).
    fn start_search(&mut self, query: String, case_sensitive: bool, use_regex: bool);
    /// Navigate to the next match.
    fn search_next(&mut self);
    /// Navigate to the previous match.
    fn search_prev(&mut self);
    /// Close search and clear highlights.
    fn close_search(&mut self);
    /// Get current match info: (current_index, total_matches).
    fn get_search_match_info(&self) -> Option<(usize, usize)>;
}

/// Panel width preference for auto-stacking into existing groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidthPreference {
    /// Prefer the narrowest existing group (sidebar-like panels).
    PreferNarrow,
    /// Prefer the widest existing group (content panels).
    PreferWide,
    /// No preference — use current (focused) group.
    NoPreference,
}

/// Visual role of a status-bar segment; mapped to theme colours by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    /// Dim label / separator text (field names like `View:`, ` │ `).
    Label,
    /// Informational value: normal colour, regular weight (e.g. `LF`, `UTF-8`).
    Value,
    /// Clickable / changeable value: normal colour, bold to signal it.
    Active,
    /// Inactive option of a toggle.
    Inactive,
    /// Warning emphasis.
    Warn,
    /// Error emphasis.
    Error,
}

/// A status-bar segment contributed by a panel via [`Panel::status_segments`].
///
/// The global status bar renders the focused panel's segments left-to-right.
/// A segment with `action = Some(id)` is a clickable chip: a click is routed
/// back to the panel through [`Panel::handle_status_action`].
#[derive(Debug, Clone)]
pub struct StatusSegment {
    /// Text to display (the panel includes its own spacing/separators).
    pub text: String,
    /// Visual role.
    pub kind: SegmentKind,
    /// `Some(id)` makes the segment a clickable chip routed to the panel.
    pub action: Option<&'static str>,
}

impl StatusSegment {
    /// Non-interactive segment.
    pub fn new(text: impl Into<String>, kind: SegmentKind) -> Self {
        Self {
            text: text.into(),
            kind,
            action: None,
        }
    }

    /// Clickable chip whose click is routed to the panel's
    /// [`Panel::handle_status_action`] with `action`.
    pub fn clickable(text: impl Into<String>, kind: SegmentKind, action: &'static str) -> Self {
        Self {
            text: text.into(),
            kind,
            action: Some(action),
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

    /// Optional per-instance header icon, overriding the by-name default
    /// (e.g. a globe for a viewer showing a fetched web page). `None` uses the
    /// panel-type icon.
    fn icon(&self) -> Option<&'static str> {
        None
    }

    /// Prepare panel for rendering (update cached theme/config).
    ///
    /// Called before render() to sync panel's internal state with current app
    /// state. `config` is borrowed so the per-frame call is a no-op for panels
    /// that ignore it and at most one `Arc::clone` for panels that cache it.
    fn prepare_render(&mut self, theme: &Theme, config: &Arc<Config>) {
        let _ = (theme, config);
    }

    /// Render the panel to the buffer.
    ///
    /// # Arguments
    /// * `area` - The area to render into
    /// * `buf` - The buffer to render to
    /// * `ctx` - Render context with theme and focus info
    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext);

    /// Handle a keyboard input event.
    ///
    /// `chord` carries both the raw `KeyEvent` from crossterm and the
    /// canonical form for hotkey matching. Use `chord.canonical` when
    /// comparing against bindings (`HotkeyTable::matches_canonical`,
    /// vim command interpretation); use `chord.raw` for text input
    /// (`InsertChar`), PTY passthrough (`modern_key_bytes`), and
    /// search-buffer typing.
    ///
    /// Returns a list of events to be processed by the application.
    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent>;

    /// Handle mouse input.
    ///
    /// # Arguments
    /// * `event` - The mouse event
    /// * `panel_area` - The panel's area (for coordinate translation)
    fn handle_mouse(&mut self, event: MouseEvent, panel_area: Rect) -> Vec<PanelEvent> {
        let _ = (event, panel_area);
        vec![]
    }

    /// Handle coalesced scroll events.
    ///
    /// Called when multiple scroll events are batched together.
    /// This is more efficient than processing individual scroll events.
    ///
    /// Optional — panels that don't need custom scroll handling can rely on
    /// the default empty implementation.
    ///
    /// # Arguments
    /// * `delta` - Combined scroll delta (positive=down, negative=up)
    /// * `panel_area` - The panel's area
    fn handle_scroll(&mut self, delta: i32, panel_area: Rect) -> Vec<PanelEvent> {
        let _ = (delta, panel_area);
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

    /// Segments this panel contributes to the global status bar.
    ///
    /// Rendered left-to-right when this panel is focused; an empty list (the
    /// default) leaves the status bar to its other content. A segment with
    /// `action = Some(id)` is a clickable chip whose click is routed back via
    /// [`Panel::handle_status_action`].
    fn status_segments(&self) -> Vec<StatusSegment> {
        vec![]
    }

    /// Handle a click on a clickable status-bar segment (by its `action` id).
    ///
    /// Returns events to be processed by the application.
    fn handle_status_action(&mut self, action: &str) -> Vec<PanelEvent> {
        let _ = action;
        vec![]
    }

    /// Extra entries this panel contributes to its `[≡]` action menu, as
    /// `(label, action_id)` pairs appended above the generic move/close items.
    /// An empty list (the default) adds nothing. A chosen entry is routed back
    /// via [`Panel::handle_status_action`] using its `action_id`.
    fn context_menu_items(&self) -> Vec<(String, &'static str)> {
        vec![]
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

    /// Get working directory as display string (includes URL for remote paths).
    /// Override this for panels that support remote paths.
    fn get_working_directory_display(&self) -> Option<String> {
        self.get_working_directory()
            .map(|p| p.display().to_string())
    }

    /// Check if there are running child processes (for terminal).
    fn has_running_processes(&self) -> bool {
        false
    }

    /// Terminate all child processes (for terminal).
    fn kill_processes(&mut self) {}

    /// Check if this is a Help panel.
    fn is_help_panel(&self) -> bool {
        false
    }

    /// Width preference for auto-stacking into existing groups.
    fn width_preference(&self) -> WidthPreference {
        WidthPreference::NoPreference
    }

    /// Colorize the truncated title for the panel header.
    ///
    /// Override this to apply per-segment coloring (e.g. git indicators).
    /// Default implementation returns the title in a single style.
    fn colorize_title(&self, truncated: &str, base_style: Style) -> Line<'static> {
        Line::from(Span::styled(truncated.to_string(), base_style))
    }
}
