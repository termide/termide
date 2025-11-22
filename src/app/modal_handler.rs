use anyhow::Result;

use super::App;
use crate::{
    state::ActiveModal,
    ui::modal::{Modal, ModalResult},
};

impl App {
    /// Handle keyboard event in modal window
    pub(super) fn handle_modal_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Get mutable reference to active modal window
        if let Some(modal) = self.state.get_active_modal_mut() {
            // Handle event in corresponding modal window
            let modal_result = match modal {
                ActiveModal::Confirm(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Input(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Select(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Overwrite(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Conflict(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Info(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::RenamePattern(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
            };

            // If modal window returned result, handle it
            if let Some(result) = modal_result {
                // Handle cancellation from RenamePattern - return to ConflictModal
                if matches!(modal, ActiveModal::RenamePattern(_))
                    && matches!(result, ModalResult::Cancelled)
                {
                    // Take operation from pending action and return to ConflictModal
                    if let Some(action) = self.state.take_pending_action() {
                        if let crate::state::PendingAction::RenameWithPattern {
                            operation, ..
                        } = action
                        {
                            use crate::ui::modal::ConflictModal;

                            if let Some(source) = operation.current_source() {
                                // Determine target path
                                let final_dest = if operation.destination.is_dir() {
                                    operation
                                        .destination
                                        .join(source.file_name().unwrap_or_default())
                                } else if operation.sources.len() == 1 {
                                    operation.destination.clone()
                                } else {
                                    operation
                                        .destination
                                        .join(source.file_name().unwrap_or_default())
                                };

                                let modal = ConflictModal::new(source, &final_dest);
                                self.state.pending_action =
                                    Some(crate::state::PendingAction::ContinueBatchOperation {
                                        operation,
                                    });
                                self.state.active_modal =
                                    Some(ActiveModal::Conflict(Box::new(modal)));
                                return Ok(());
                            }
                        }
                    }
                }

                self.state.close_modal();
                if let ModalResult::Confirmed(value) = result {
                    self.handle_modal_result(value)?;
                }
            }
        }
        Ok(())
    }

    /// Handle modal window result
    pub(super) fn handle_modal_result(&mut self, value: Box<dyn std::any::Any>) -> Result<()> {
        if let Some(action) = self.state.take_pending_action() {
            match action {
                crate::state::PendingAction::CreateFile {
                    panel_index,
                    directory,
                } => {
                    self.handle_create_file(panel_index, directory, value)?;
                }
                crate::state::PendingAction::CreateDirectory {
                    panel_index,
                    directory,
                } => {
                    self.handle_create_directory(panel_index, directory, value)?;
                }
                crate::state::PendingAction::DeletePath { panel_index, paths } => {
                    self.handle_delete_path(panel_index, paths, value)?;
                }
                crate::state::PendingAction::SaveFileAs {
                    panel_index,
                    directory,
                } => {
                    self.handle_save_file_as(panel_index, directory, value)?;
                }
                crate::state::PendingAction::ClosePanel { panel_index } => {
                    self.handle_close_panel(panel_index, value)?;
                }
                crate::state::PendingAction::CloseEditorWithSave { panel_index } => {
                    self.handle_close_editor_with_save(panel_index, value)?;
                }
                crate::state::PendingAction::OverwriteDecision {
                    panel_index,
                    source,
                    destination,
                    is_move,
                } => {
                    self.handle_overwrite_decision(
                        panel_index,
                        source,
                        destination,
                        is_move,
                        value,
                    )?;
                }
                crate::state::PendingAction::CopyPath {
                    panel_index,
                    sources,
                    target_directory,
                } => {
                    self.handle_copy_path(panel_index, sources, target_directory, value)?;
                }
                crate::state::PendingAction::MovePath {
                    panel_index,
                    sources,
                    target_directory,
                } => {
                    self.handle_move_path(panel_index, sources, target_directory, value)?;
                }
                crate::state::PendingAction::BatchFileOperation { operation } => {
                    self.process_batch_operation(operation.clone());
                }
                crate::state::PendingAction::ContinueBatchOperation { operation } => {
                    self.handle_continue_batch_operation(operation, value)?;
                }
                crate::state::PendingAction::RenameWithPattern {
                    operation,
                    original_name,
                } => {
                    self.handle_rename_with_pattern(operation, original_name, value)?;
                }
                crate::state::PendingAction::Search => {
                    self.handle_search(value)?;
                }
                crate::state::PendingAction::Replace => {
                    self.handle_replace(value)?;
                }
                crate::state::PendingAction::ReplaceStep2 { query } => {
                    self.handle_replace_step2(query, value)?;
                }
                crate::state::PendingAction::QuitApplication => {
                    // User confirmed quit - exit application
                    self.state.quit();
                }
                // Navigation actions are handled in key_handler, should not get here
                crate::state::PendingAction::NextPanel | crate::state::PendingAction::PrevPanel => {
                }
            }
        }
        Ok(())
    }

    /// Handle search result
    fn handle_search(&mut self, value: Box<dyn std::any::Any>) -> Result<()> {
        use crate::panels::editor::Editor;

        if let Some(query) = value.downcast_ref::<String>() {
            // Get active panel
            let panel_index = self.state.active_panel;
            if let Some(panel) = self.panels.get_mut(panel_index) {
                // Try to downcast to Editor
                if let Some(editor) = (panel as &mut dyn std::any::Any).downcast_mut::<Editor>() {
                    // Start search (case insensitive by default)
                    editor.start_search(query.clone(), false);
                }
            }
        }
        Ok(())
    }

    /// Handle replace result (first step - search query)
    fn handle_replace(&mut self, value: Box<dyn std::any::Any>) -> Result<()> {
        use crate::ui::modal::InputModal;

        if let Some(query) = value.downcast_ref::<String>() {
            // Save query and show second modal window for replacement
            let input = InputModal::new("Replace", "Replace with:");
            self.state.pending_action = Some(crate::state::PendingAction::ReplaceStep2 {
                query: query.clone(),
            });
            self.state.active_modal = Some(ActiveModal::Input(Box::new(input)));
        }
        Ok(())
    }

    /// Handle second step of replace (entering replacement string)
    fn handle_replace_step2(&mut self, query: String, value: Box<dyn std::any::Any>) -> Result<()> {
        use crate::panels::editor::Editor;

        if let Some(replace_with) = value.downcast_ref::<String>() {
            // Get active panel
            let panel_index = self.state.active_panel;
            if let Some(panel) = self.panels.get_mut(panel_index) {
                // Try to downcast to Editor
                if let Some(editor) = (panel as &mut dyn std::any::Any).downcast_mut::<Editor>() {
                    // Start replace (case insensitive by default)
                    editor.start_replace(query, replace_with.clone(), false);
                }
            }
        }
        Ok(())
    }
}
