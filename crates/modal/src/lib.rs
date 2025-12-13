//! Modal dialog system for termide.
//!
//! Provides themed modal dialogs for user interaction.
//! Uses termide-ui for base utilities and termide-theme for styling.

use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use termide_theme::Theme;

// Re-export modal utilities from termide-ui
pub use termide_ui::{
    calculate_modal_width, centered_rect_with_size, max_item_width, max_line_width, ModalResult,
    ModalWidthConfig, TextInput as TextInputHandler,
};

pub mod base;
pub mod confirm;
pub mod conflict;
pub mod editable_select;
pub mod info;
pub mod input;
pub mod overwrite;
pub mod rename_pattern;
pub mod replace;
pub mod search;
pub mod select;

pub use confirm::ConfirmModal;
pub use conflict::{ConflictModal, ConflictResolution};
pub use editable_select::{EditableSelectModal, SelectOption};
pub use info::InfoModal;
pub use input::InputModal;
pub use overwrite::{OverwriteChoice, OverwriteModal};
pub use rename_pattern::RenamePatternModal;
pub use replace::{ReplaceAction, ReplaceModal, ReplaceModalResult};
pub use search::{SearchAction, SearchModal, SearchModalResult};
pub use select::SelectModal;

/// Active modal window enum.
///
/// Contains all possible modal types in boxed form for dynamic dispatch.
#[derive(Debug)]
pub enum ActiveModal {
    /// Confirmation modal (Yes/No)
    Confirm(Box<ConfirmModal>),
    /// Text input modal
    Input(Box<InputModal>),
    /// Selection modal (single selection)
    Select(Box<SelectModal>),
    /// File overwrite modal
    #[allow(dead_code)]
    Overwrite(Box<OverwriteModal>),
    /// File conflict resolution modal
    Conflict(Box<ConflictModal>),
    /// Information modal
    Info(Box<InfoModal>),
    /// Rename pattern input modal
    RenamePattern(Box<RenamePatternModal>),
    /// Editable select modal (combobox)
    EditableSelect(Box<EditableSelectModal>),
    /// Interactive search modal
    Search(Box<SearchModal>),
    /// Interactive replace modal
    Replace(Box<ReplaceModal>),
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
    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>>;

    /// Handle mouse event.
    /// Returns Some(result) if the modal window should close.
    fn handle_mouse(
        &mut self,
        _mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        Ok(None) // Default: do nothing
    }
}
