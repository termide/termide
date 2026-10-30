//! Turning editing off over unsaved work, in the text and the hex editor:
//! the work is saved, discarded, or editing goes on.

use std::path::{Path, PathBuf};

use super::App;
use crate::state::{ActiveModal, PendingAction};
use termide_i18n as i18n;
use termide_panel_binary::BinaryPanel;

/// The answers, in the order the question lists them.
const SAVE: usize = 0;
const DISCARD: usize = 1;

impl App {
    /// Ask what becomes of the unsaved work in the editor of `path` before
    /// editing turns off.
    pub(super) fn ask_stop_editing(&mut self, path: PathBuf) {
        let t = i18n::t();
        let modal = termide_modal::ChoiceModal::new(
            t.editor_stop_editing(),
            Some(t.editor_close_unsaved_question().to_string()),
            vec![
                t.editor_save_and_view().to_string(),
                t.editor_view_without_saving().to_string(),
                t.editor_cancel().to_string(),
            ],
        );
        self.state.set_pending_action(
            PendingAction::StopEditing { path },
            ActiveModal::Choice(Box::new(modal)),
        );
        self.state.needs_redraw = true;
    }

    /// The answer about the unsaved work in the editor of `path`.
    pub(super) fn stop_editing(&mut self, path: &Path, choice: usize) {
        if choice != SAVE && choice != DISCARD {
            return;
        }
        self.state.needs_redraw = true;
        let project = self.open_projects.current().to_path_buf();
        if let Some(editor) = self.project_editor_mut(&project, path) {
            if choice == DISCARD {
                if let Err(e) = editor.reload_from_disk() {
                    self.show_error_modal(format!("Reload failed: {e}"));
                    return;
                }
                editor.set_editable(false);
            } else if editor.is_remote() {
                match editor.save() {
                    Ok(upload) => {
                        editor.set_editable(false);
                        if let Some((temp_path, remote_path, vfs_manager)) = upload {
                            self.queue_remote_editor_upload(
                                temp_path,
                                remote_path,
                                vfs_manager,
                                None,
                            );
                        }
                    }
                    Err(e) => {
                        let t = i18n::t();
                        self.show_error_modal(t.status_error_save(&e.to_string()));
                    }
                }
            } else {
                self.save_then_view(&project, path);
            }
            return;
        }
        let Some(hex) = self
            .layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_any_mut().downcast_mut::<BinaryPanel>())
            .find(|hex| hex.file_path() == path)
        else {
            return;
        };
        let saved = if choice == DISCARD {
            hex.discard_edits();
            Ok(None)
        } else {
            hex.save()
        };
        if saved.is_ok() {
            hex.set_editable(false);
        }
        match saved {
            Ok(Some(backup)) => {
                let name = backup
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.state.set_info(format!("Saved (backup: {name})"));
            }
            Ok(None) => {}
            Err(e) => self.show_error_modal(format!("Save failed: {e}")),
        }
    }
}
