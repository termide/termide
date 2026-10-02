//! Modal dialog system for termide.
//!
//! Provides themed modal dialogs for user interaction.
//! Uses termide-ui for base utilities and termide-theme for styling.

use anyhow::Result;
use ratatui::{buffer::Buffer, layout::Rect};

use termide_theme::Theme;

// Re-export modal utilities from termide-ui
pub use termide_ui::{
    calculate_modal_width, centered_rect_with_size, fit_modal_width, max_item_width,
    max_line_width, ModalResult, ModalWidthConfig, TextInput as TextInputHandler,
};

pub mod base;
pub mod input_keys;
pub use base::{
    check_mouse_click, check_mouse_click_with_item_height, is_click_outside, CursorNavigation,
    MouseClickResult,
};
pub use input_keys::{handle_input_key, InputKeyResult};
pub mod bookmark_add;
pub mod calendar;
pub mod checklist;
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
pub mod projects;
pub mod rename_pattern;
pub mod save_as;
pub mod select;
pub mod settings;

pub use bookmark_add::{BookmarkAddModal, BookmarkAddResult};
pub use calendar::CalendarModal;
pub use checklist::ChecklistModal;
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
pub use input::{InputModal, Suggest};
pub use projects::{ProjectAction, ProjectItem, ProjectsModal};
pub use rename_pattern::RenamePatternModal;
pub use save_as::{SaveAsModal, SaveAsResult};
pub use select::SelectModal;
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
    /// Checkboxes under group headings, applied together
    Checklist(Box<ChecklistModal>),
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
    /// Projects selection modal
    Projects(Box<ProjectsModal>),
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
            ActiveModal::Checklist(m) => m.$method($($arg),*),
            ActiveModal::Conflict(m) => m.$method($($arg),*),
            ActiveModal::Info(m) => m.$method($($arg),*),
            ActiveModal::InfoAction(m) => m.$method($($arg),*),
            ActiveModal::RenamePattern(m) => m.$method($($arg),*),
            ActiveModal::EditableSelect(m) => m.$method($($arg),*),
            ActiveModal::Projects(m) => m.$method($($arg),*),
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
            ActiveModal::Checklist(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Conflict(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Info(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::InfoAction(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::RenamePattern(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::EditableSelect(m) => m.$method($($arg),*)?.map(erase_modal_result),
            ActiveModal::Projects(m) => m.$method($($arg),*)?.map(erase_modal_result),
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

    /// Bring the open checklist up to date with what its panel knows now;
    /// `false` when the modal is not a checklist, or nothing changed.
    pub fn refresh_checklist(
        &mut self,
        items: Vec<termide_core::ChecklistItem>,
        groups: Vec<termide_core::ChecklistGroup>,
        prompt: Option<String>,
    ) -> bool {
        match self {
            ActiveModal::Checklist(m) => m.refresh(items, groups, prompt),
            _ => false,
        }
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

#[cfg(test)]
mod outside_click_tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::buffer::Buffer;
    use termide_theme::Theme;

    fn press(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Render `modal` centred on a large screen, then report whether a click
    /// in the corner (beside it) closes it — by cancelling, or, for a
    /// checklist, by applying what was chosen — and whether one at the centre
    /// (on it) cancels it.
    fn clicks<M: Modal>(mut modal: M) -> (bool, bool) {
        let screen = Rect::new(0, 0, 120, 40);
        let before = modal.handle_mouse(press(0, 0), screen).unwrap();
        assert!(before.is_none(), "no frame yet, nothing to be beside");
        let mut buf = Buffer::empty(screen);
        modal.render(screen, &mut buf, &Theme::default());
        let beside = modal.handle_mouse(press(0, 0), screen).unwrap().is_some();
        // On it, a click may pick (a select's option), but never dismisses.
        let on = matches!(
            modal.handle_mouse(press(60, 20), screen).unwrap(),
            Some(ModalResult::Cancelled)
        );
        (beside, on)
    }

    #[test]
    fn a_click_beside_a_modal_dismisses_it_and_one_on_it_does_not() {
        let item = termide_core::ChecklistItem {
            key: "read".into(),
            label: "read".into(),
            group: String::new(),
            checked: true,
            enabled: true,
            note: String::new(),
        };
        let results = [
            (
                "select",
                clicks(SelectModal::single(
                    "Pick",
                    "",
                    vec!["a".into(), "b".into()],
                )),
            ),
            (
                "checklist",
                clicks(ChecklistModal::new("Tools", "", vec![item], vec![])),
            ),
            ("input", clicks(InputModal::new("Name", "Enter a name"))),
            ("confirm", clicks(ConfirmModal::new("Delete", "Sure?"))),
            (
                "info",
                clicks(InfoModal::new("Info", vec![("key".into(), "value".into())])),
            ),
        ];
        for (name, (beside, on)) in results {
            assert!(beside, "{name}: a click beside it dismisses it");
            assert!(!on, "{name}: a click on it does not");
        }
    }
}
