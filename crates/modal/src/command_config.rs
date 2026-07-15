//! Unified modal dialog for creating and editing commands.
//!
//! In both modes the Group, Display Name, Command, Mode, Hotkey, and Project
//! fields are editable.

use ratatui::layout::Rect;

use termide_config::commands::CommandMode;
use termide_config::ParsedKeyBinding;
use termide_ui::SuggestionInput;

use crate::TextInputHandler;

mod render;
mod state;
mod view;

/// Sanitize a string for use as filename/directory name.
pub fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '.' => '-',
            _ => c,
        })
        .collect()
}

/// Modal mode: creating a new command or editing an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandConfigMode {
    Create,
    Edit,
}

/// Action the user chose when confirming the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandConfigAction {
    Save,
}

/// Result returned by the modal on confirmation.
#[derive(Debug, Clone)]
pub struct CommandConfigResult {
    pub name: String,
    pub command: Option<String>,
    pub display_name: Option<String>,
    pub group: Option<String>,
    pub mode: CommandMode,
    pub hotkey: Option<String>,
    pub is_project: bool,
    pub action: CommandConfigAction,
    /// Whether this is an edit (Some) or create (None).
    pub is_edit: bool,
}

#[derive(Debug, Clone)]
pub struct ReservedHotkey {
    pub binding: String,
}

/// Focus area in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Group,
    Command,
    DisplayName,
    Mode,
    Hotkey,
    ProjectCheckbox,
    Buttons,
}

/// Command configuration modal (unified create/edit).
#[derive(Debug)]
pub struct CommandConfigModal {
    title: String,
    mode: CommandConfigMode,
    focus: FocusArea,
    // Edit-mode: stored command name (TOML key, not editable)
    command_name: String,
    // Fields
    group_suggestion: SuggestionInput,
    command_input: TextInputHandler,
    display_name_input: TextInputHandler,
    command_mode: CommandMode,
    hotkey_input: TextInputHandler,
    is_project: bool,
    // Button selection: 0=Save/Create, 1=Cancel
    selected_button: usize,
    // Validation state
    hotkey_error: bool,
    hotkey_conflict: Option<String>,
    reserved_hotkeys: Vec<ParsedKeyBinding>,
    // Cached areas for mouse handling
    last_buttons_area: Option<Rect>,
    last_group_field_area: Option<Rect>,
    last_group_dropdown_area: Option<Rect>,
    last_command_area: Option<Rect>,
    last_display_name_area: Option<Rect>,
    last_mode_area: Option<Rect>,
    last_hotkey_area: Option<Rect>,
    last_checkbox_area: Option<Rect>,
}
