use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::theme::Theme;

pub mod confirm;
pub mod conflict;
pub mod editable_select;
pub mod info;
pub mod input;
pub mod overwrite;
pub mod rename_pattern;
pub mod select;

pub use confirm::ConfirmModal;
pub use conflict::{ConflictModal, ConflictResolution};
pub use editable_select::{EditableSelectModal, SelectOption};
pub use info::InfoModal;
pub use input::InputModal;
pub use overwrite::{OverwriteChoice, OverwriteModal};
pub use rename_pattern::RenamePatternModal;
pub use select::SelectModal;

/// Modal window result
#[derive(Debug, Clone)]
pub enum ModalResult<T> {
    /// User confirmed the action with a result
    Confirmed(T),
    /// User cancelled the action
    Cancelled,
}

/// Trait for all modal windows
pub trait Modal {
    /// Modal window result type
    type Result;

    /// Render the modal window
    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme);

    /// Handle keyboard event
    /// Returns Some(result) if the modal window should close
    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<ModalResult<Self::Result>>>;

    /// Handle mouse event
    /// Returns Some(result) if the modal window should close
    fn handle_mouse(
        &mut self,
        _mouse: crossterm::event::MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        Ok(None) // Default: do nothing
    }
}
