//! Panel extension traits for downcasting.
//!
//! # Deprecation Notice
//!
//! This trait is **deprecated** in favor of the `handle_command()` method on `Panel`.
//!
//! Instead of downcasting to concrete panel types, use `Panel::handle_command()`
//! with appropriate `PanelCommand` variants:
//!
//! ```rust,ignore
//! // Old approach (deprecated):
//! if let Some(editor) = panel.as_editor_mut() {
//!     editor.update_git_diff();
//! }
//!
//! // New approach (preferred):
//! panel.handle_command(PanelCommand::OnGitUpdate { repo_paths: &paths });
//! ```
//!
//! # When PanelExt is still used
//!
//! Some operations intentionally remain using PanelExt because they don't fit
//! the command pattern well:
//!
//! - **Resource extraction**: `take_config_update()`, `dir_size_receiver.take()`
//! - **Complex type-specific methods**: `go_to_line()`, `save_as()`, batch operations
//! - **Modal requests**: `take_modal_request()` (returns concrete types)
//!
//! These will be reviewed for potential migration in future versions.

use std::any::Any;

use termide_core::Panel;
use termide_modal::ActiveModal;
use termide_panel_editor::Editor;
use termide_panel_file_manager::FileManager;
use termide_panel_misc::LogViewerPanel;
use termide_panel_terminal::Terminal;
use termide_state::PendingAction;

/// Extension trait for convenient downcasting of Panel trait objects.
///
/// # Deprecated
///
/// This trait is deprecated. Use `Panel::handle_command()` with `PanelCommand` instead.
/// See module documentation for migration examples.
// Allow deprecated use within this module for internal implementation
#[allow(deprecated)]
#[deprecated(
    since = "0.5.0",
    note = "Use Panel::handle_command() with PanelCommand variants instead"
)]
pub trait PanelExt {
    /// Downcast to Editor (immutable)
    fn as_editor(&self) -> Option<&Editor>;
    /// Downcast to Editor (mutable)
    fn as_editor_mut(&mut self) -> Option<&mut Editor>;
    /// Downcast to FileManager (mutable)
    fn as_file_manager_mut(&mut self) -> Option<&mut FileManager>;
    /// Downcast to Terminal (mutable)
    fn as_terminal_mut(&mut self) -> Option<&mut Terminal>;
    /// Check if panel is a LogViewer
    fn is_log_viewer(&self) -> bool;
    /// Take modal request from FileManager or Editor panels.
    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)>;
}

#[allow(deprecated)]
impl PanelExt for dyn Panel {
    fn as_editor(&self) -> Option<&Editor> {
        (self as &dyn Any).downcast_ref::<Editor>()
    }

    fn as_editor_mut(&mut self) -> Option<&mut Editor> {
        (self as &mut dyn Any).downcast_mut::<Editor>()
    }

    fn as_file_manager_mut(&mut self) -> Option<&mut FileManager> {
        (self as &mut dyn Any).downcast_mut::<FileManager>()
    }

    fn as_terminal_mut(&mut self) -> Option<&mut Terminal> {
        (self as &mut dyn Any).downcast_mut::<Terminal>()
    }

    fn is_log_viewer(&self) -> bool {
        (self as &dyn Any).is::<LogViewerPanel>()
    }

    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        if let Some(fm) = self.as_file_manager_mut() {
            return fm.take_modal_request();
        }
        if let Some(editor) = self.as_editor_mut() {
            return editor.take_modal_request();
        }
        None
    }
}

#[allow(deprecated)]
impl PanelExt for Box<dyn Panel> {
    fn as_editor(&self) -> Option<&Editor> {
        (**self).as_editor()
    }

    fn as_editor_mut(&mut self) -> Option<&mut Editor> {
        (**self).as_editor_mut()
    }

    fn as_file_manager_mut(&mut self) -> Option<&mut FileManager> {
        (**self).as_file_manager_mut()
    }

    fn as_terminal_mut(&mut self) -> Option<&mut Terminal> {
        (**self).as_terminal_mut()
    }

    fn is_log_viewer(&self) -> bool {
        (**self).is_log_viewer()
    }

    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        (**self).take_modal_request()
    }
}
