//! Modal dialog system for termide.
//!
//! Provides themed modal dialogs for user interaction.
//! Uses termide-ui for base utilities and termide-theme for styling.

use anyhow::Result;
use ratatui::{buffer::Buffer, layout::Rect};

use termide_theme::Theme;

// Re-export modal utilities from termide-ui
pub use termide_ui::{
    calculate_modal_width, centered_rect_with_size, max_item_width, max_line_width, ModalResult,
    ModalWidthConfig, TextInput as TextInputHandler,
};

pub mod base;
pub mod input_keys;
pub use base::{
    check_mouse_click, check_mouse_click_with_item_height, CursorNavigation, MouseClickResult,
};
pub use input_keys::{handle_input_key, InputKeyResult};
pub mod bookmark_add;
pub mod calendar;
pub mod choice;
pub mod command_config;
pub mod command_palette;
pub mod command_params;
pub mod commit;
pub mod confirm;
pub mod conflict;
pub mod db_filter;
pub mod db_row_edit;
pub mod directory_picker;
pub mod directory_switcher;
pub mod editable_select;
pub mod find_bar;
pub mod info;
pub mod info_action;
pub mod input;
pub mod rename_pattern;
pub mod save_as;
pub mod select;
pub mod sessions;
pub mod settings;

pub use bookmark_add::{BookmarkAddModal, BookmarkAddResult};
pub use calendar::CalendarModal;
pub use choice::ChoiceModal;
pub use command_config::{
    sanitize_filename, CommandConfigAction, CommandConfigModal, CommandConfigMode,
    CommandConfigResult, ReservedHotkey,
};
pub use command_palette::{CommandEntry, CommandPaletteModal};
pub use command_params::{CommandParamsModal, CommandParamsResult};
pub use commit::CommitModal;
pub use confirm::ConfirmModal;
pub use conflict::{ConflictModal, ConflictResolution};
pub use db_filter::{DbFilterColumn, DbFilterCondition, DbFilterModal, DbFilterResult};
pub use db_row_edit::{DbRowEditColumn, DbRowEditModal, DbRowEditResult};
pub use directory_picker::DirectoryPickerModal;
pub use directory_switcher::{DirectoryItem, DirectorySwitcherModal};
pub use editable_select::{EditableSelectModal, SelectOption};
pub use find_bar::{Btn as FindBarBtn, FindBar, FindBarAction, FindBarConfig, FindField};
pub use info::{InfoModal, ModalValue, SegmentStyle, StyledSegment};
pub use info_action::{
    ActionButton, InfoActionModal, InfoActionResult, PermAccess, PermissionsState,
};
pub use input::InputModal;
pub use rename_pattern::RenamePatternModal;
pub use save_as::{SaveAsModal, SaveAsResult};
pub use select::SelectModal;
pub use sessions::{SessionAction, SessionItem, SessionsModal};
pub use settings::{SettingsModal, SettingsResult};

/// Active modal window enum.
///
/// Contains all possible modal types in boxed form for dynamic dispatch.
#[derive(Debug)]
pub enum ActiveModal {
    /// Git commit modal
    Commit(Box<CommitModal>),
    /// Confirmation modal (Yes/No)
    Confirm(Box<ConfirmModal>),
    /// Choice modal with horizontal buttons
    Choice(Box<ChoiceModal>),
    /// Text input modal
    Input(Box<InputModal>),
    /// Selection modal (single selection)
    Select(Box<SelectModal>),
    /// File conflict resolution modal
    Conflict(Box<ConflictModal>),
    /// Information modal
    Info(Box<InfoModal>),
    /// Information modal with action buttons
    InfoAction(Box<InfoActionModal>),
    /// Rename pattern input modal
    RenamePattern(Box<RenamePatternModal>),
    /// Editable select modal (combobox)
    EditableSelect(Box<EditableSelectModal>),
    /// Sessions selection modal
    Sessions(Box<SessionsModal>),
    /// Directory picker modal
    DirectoryPicker(Box<DirectoryPickerModal>),
    /// Save As modal with executable checkbox
    SaveAs(Box<SaveAsModal>),
    /// Directory switcher modal
    DirectorySwitcher(Box<DirectorySwitcherModal>),
    /// Bookmark add modal
    BookmarkAdd(Box<BookmarkAddModal>),
    /// Calendar modal
    Calendar(Box<CalendarModal>),
    /// Command palette modal
    CommandPalette(Box<CommandPaletteModal>),
    /// Command config modal (unified create/edit)
    CommandConfig(Box<CommandConfigModal>),
    /// Command parameters form modal
    CommandParams(Box<CommandParamsModal>),
    /// Settings modal with tabbed interface
    Settings(Box<SettingsModal>),
    /// Database single-column filter modal
    DbFilter(Box<DbFilterModal>),
    DbRowEdit(Box<DbRowEditModal>),
}

/// Helper to convert a typed ModalResult into a type-erased ModalResult<Box<dyn Any>>.
fn erase_modal_result<T: 'static>(result: ModalResult<T>) -> ModalResult<Box<dyn std::any::Any>> {
    match result {
        ModalResult::Confirmed(value) => {
            ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
        }
        ModalResult::Cancelled => ModalResult::Cancelled,
    }
}

/// Dispatch a method call to the inner modal across all ActiveModal variants.
macro_rules! dispatch_modal {
    ($self:expr, $method:ident $(, $arg:expr)*) => {
        match $self {
            ActiveModal::Commit(m) => m.$method($($arg),*),
            ActiveModal::Confirm(m) => m.$method($($arg),*),
            ActiveModal::Choice(m) => m.$method($($arg),*),
            ActiveModal::Input(m) => m.$method($($arg),*),
            ActiveModal::Select(m) => m.$method($($arg),*),
            ActiveModal::Conflict(m) => m.$method($($arg),*),
            ActiveModal::Info(m) => m.$method($($arg),*),
            ActiveModal::InfoAction(m) => m.$method($($arg),*),
            ActiveModal::RenamePattern(m) => m.$method($($arg),*),
            ActiveModal::EditableSelect(m) => m.$method($($arg),*),
            ActiveModal::Sessions(m) => m.$method($($arg),*),
            ActiveModal::DirectoryPicker(m) => m.$method($($arg),*),
            ActiveModal::SaveAs(m) => m.$method($($arg),*),
            ActiveModal::DirectorySwitcher(m) => m.$method($($arg),*),
            ActiveModal::BookmarkAdd(m) => m.$method($($arg),*),
            ActiveModal::Calendar(m) => m.$method($($arg),*),
            ActiveModal::CommandPalette(m) => m.$method($($arg),*),
            ActiveModal::CommandConfig(m) => m.$method($($arg),*),
            ActiveModal::CommandParams(m) => m.$method($($arg),*),
            ActiveModal::Settings(m) => m.$method($($arg),*),
            ActiveModal::DbFilter(m) => m.$method($($arg),*),
            ActiveModal::DbRowEdit(m) => m.$method($($arg),*),
        }
    };
}

/// Dispatch handle_key/handle_mouse and erase the result type.
macro_rules! dispatch_modal_erased {
    ($self:expr, $method:ident $(, $arg:expr)*) => {
        match $self {
            ActiveModal::Commit(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Confirm(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Choice(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Input(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Select(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Conflict(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Info(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::InfoAction(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::RenamePattern(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::EditableSelect(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Sessions(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::DirectoryPicker(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::SaveAs(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::DirectorySwitcher(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::BookmarkAdd(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Calendar(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::CommandPalette(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::CommandConfig(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::CommandParams(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Settings(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::DbFilter(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::DbRowEdit(m) => m.$method($($arg),*)?.map(erase_modal_result),
        }
    };
}

impl ActiveModal {
    /// Handle keyboard event, returning type-erased result.
    pub fn handle_key_erased(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Box<dyn std::any::Any>>>> {
        Ok(dispatch_modal_erased!(self, handle_key, chord))
    }

    /// Handle mouse event, returning type-erased result.
    pub fn handle_mouse_erased(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        modal_area: Rect,
    ) -> Result<Option<ModalResult<Box<dyn std::any::Any>>>> {
        Ok(dispatch_modal_erased!(
            self,
            handle_mouse,
            mouse,
            modal_area
        ))
    }

    /// Render the modal.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        dispatch_modal!(self, render, area, buf, theme);
    }

    /// Handle paste event.
    pub fn handle_paste(&mut self, text: &str) -> bool {
        dispatch_modal!(self, handle_paste, text)
    }
}

/// Trait for all modal windows.
///
/// This extends the base Modal concept with Theme support.
pub trait Modal {
    /// Modal window result type.
    type Result;

    /// Render the modal window with theme.
    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme);

    /// Handle keyboard event.
    /// Returns Some(result) if the modal window should close.
    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>>;

    /// Handle mouse event.
    /// Returns Some(result) if the modal window should close.
    fn handle_mouse(
        &mut self,
        _mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        Ok(None) // Default: do nothing
    }

    /// Handle paste event.
    /// Returns true if the modal handled the paste, false to pass to panel.
    fn handle_paste(&mut self, _text: &str) -> bool {
        false // Default: modals don't handle paste
    }
}
