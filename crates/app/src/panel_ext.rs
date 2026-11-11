//! Panel extension traits for downcasting to a concrete panel type.
//!
//! # How this relates to `PanelCommand`
//!
//! The two are complements, not rivals, and the split is deliberate:
//!
//! - **`Panel::handle_command`** carries operations every panel can answer —
//!   `Save`, `Reload`, `Copy`, `Resize`, `OnGitUpdate`, `GetScrollBars`. A
//!   caller that does not care which panel is focused goes through it.
//! - **`PanelExt`** reaches a specific panel's own API — `Editor::init_lsp`,
//!   `Editor::poll_completion`, `Editor::set_symbol_lines`,
//!   `FileManager::create_file`. These are features of one panel type, not
//!   contracts every panel implements.
//!
//! This trait once carried `#[deprecated]`, on the plan that everything would
//! move to `PanelCommand`. That plan does not survive contact with the
//! numbers: the app calls 67 distinct `Editor` methods, 24 on `FileManager`
//! and 5 on `Terminal` through these downcasts. Expressing them as commands
//! would take roughly 96 new variants against the 25 `PanelCommand` has
//! today, and all 96 would be panel-specific — which is precisely what a
//! shared contract is not for. The deprecation was removed rather than
//! carried as a warning nothing intended to act on.
//!
//! What still belongs in `PanelCommand`, and should be added there rather
//! than reached through a downcast:
//!
//! - anything a second panel type would plausibly answer
//! - anything the caller invokes without knowing the panel's type
//!
//! ```rust,ignore
//! // Cross-panel concern -> command:
//! panel.handle_command(PanelCommand::OnGitUpdate { repo_paths: &paths });
//!
//! // One panel's own feature -> downcast:
//! if let Some(editor) = panel.as_editor_mut() {
//!     editor.init_lsp(lsp_manager);
//! }
//! ```

use termide_core::Panel;
use termide_modal::ActiveModal;
use termide_panel_db::DbPanel;
use termide_panel_diagnostics::DiagnosticsPanel;
use termide_panel_editor::Editor;
use termide_panel_file_manager::FileManager;
use termide_panel_git_log::GitLogPanel;
use termide_panel_git_status::GitStatusPanel;
use termide_panel_misc::JournalPanel;
use termide_panel_terminal::Terminal;
use termide_state::PendingAction;

/// Extension trait for convenient downcasting of Panel trait objects.
///
/// See the module documentation for when to add a `PanelCommand` variant
/// instead of a downcast here.
pub trait PanelExt {
    /// Downcast to Editor (immutable)
    fn as_editor(&self) -> Option<&Editor>;
    /// Downcast to Editor (mutable)
    fn as_editor_mut(&mut self) -> Option<&mut Editor>;
    /// Downcast to FileManager (mutable)
    fn as_file_manager_mut(&mut self) -> Option<&mut FileManager>;
    /// Downcast to Terminal (mutable)
    fn as_terminal_mut(&mut self) -> Option<&mut Terminal>;
    /// Downcast to DiagnosticsPanel (mutable)
    fn as_diagnostics_panel_mut(&mut self) -> Option<&mut DiagnosticsPanel>;
    /// Check if panel is a Journal panel
    fn is_journal(&self) -> bool;
    /// Take modal request from FileManager, Editor, or GitStatusPanel.
    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)>;

    /// Take pending upload operation from Editor.
    /// Returns (temp_path, remote_path, vfs_manager) for app to create upload via OperationManager
    fn take_pending_upload(
        &mut self,
    ) -> Option<(
        std::path::PathBuf,
        termide_vfs::VfsPath,
        std::sync::Arc<termide_vfs::VfsManager>,
    )>;
}

impl PanelExt for dyn Panel {
    fn as_editor(&self) -> Option<&Editor> {
        self.downcast_ref::<Editor>()
    }

    fn as_editor_mut(&mut self) -> Option<&mut Editor> {
        self.downcast_mut::<Editor>()
    }

    fn as_file_manager_mut(&mut self) -> Option<&mut FileManager> {
        self.downcast_mut::<FileManager>()
    }

    fn as_terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.downcast_mut::<Terminal>()
    }

    fn as_diagnostics_panel_mut(&mut self) -> Option<&mut DiagnosticsPanel> {
        self.downcast_mut::<DiagnosticsPanel>()
    }

    fn is_journal(&self) -> bool {
        self.is::<JournalPanel>()
    }

    /// Collect a pending modal request from whichever panel type has one.
    ///
    /// Downcasts inline for the panels no caller reaches on its own, so the
    /// trait exposes only the accessors something outside this module uses.
    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        if let Some(fm) = self.as_file_manager_mut() {
            return fm.take_modal_request();
        }
        if let Some(editor) = self.as_editor_mut() {
            return editor.take_modal_request();
        }
        if let Some(git_status) = self.downcast_mut::<GitStatusPanel>() {
            return git_status.take_modal_request();
        }
        if let Some(git_log) = self.downcast_mut::<GitLogPanel>() {
            return git_log.take_modal_request();
        }
        if let Some(journal) = self.downcast_mut::<JournalPanel>() {
            return journal.editor_mut().take_modal_request();
        }
        if let Some(db) = self.downcast_mut::<DbPanel>() {
            return db.take_modal_request();
        }
        None
    }

    fn take_pending_upload(
        &mut self,
    ) -> Option<(
        std::path::PathBuf,
        termide_vfs::VfsPath,
        std::sync::Arc<termide_vfs::VfsManager>,
    )> {
        if let Some(editor) = self.as_editor_mut() {
            return editor.take_pending_upload();
        }
        None
    }
}

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

    fn as_diagnostics_panel_mut(&mut self) -> Option<&mut DiagnosticsPanel> {
        (**self).as_diagnostics_panel_mut()
    }

    fn is_journal(&self) -> bool {
        (**self).is_journal()
    }

    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        (**self).take_modal_request()
    }

    fn take_pending_upload(
        &mut self,
    ) -> Option<(
        std::path::PathBuf,
        termide_vfs::VfsPath,
        std::sync::Arc<termide_vfs::VfsManager>,
    )> {
        (**self).take_pending_upload()
    }
}
