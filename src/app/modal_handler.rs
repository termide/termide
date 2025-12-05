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
                ActiveModal::EditableSelect(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Search(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Replace(m) => m.handle_key(key)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
            };

            // If modal window returned result, handle it
            if let Some(result) = modal_result {
                // Check modal type before taking state references
                let is_rename_pattern = matches!(modal, ActiveModal::RenamePattern(_));
                let is_search = matches!(modal, ActiveModal::Search(_));
                let is_replace = matches!(modal, ActiveModal::Replace(_));

                // Handle cancellation from RenamePattern - return to ConflictModal
                if is_rename_pattern && matches!(result, ModalResult::Cancelled) {
                    // Take operation from pending action and return to ConflictModal
                    #[allow(clippy::collapsible_match)]
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

                                let remaining_items = operation
                                    .sources
                                    .len()
                                    .saturating_sub(operation.current_index + 1);
                                let modal =
                                    ConflictModal::new(source, &final_dest, remaining_items);
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

                // Handle SearchModal specially - don't close on navigation
                if is_search {
                    if let ModalResult::Confirmed(value) = &result {
                        if let Some(search_result) =
                            value.downcast_ref::<crate::ui::modal::SearchModalResult>()
                        {
                            // Handle search action in editor
                            self.handle_search_action(search_result)?;

                            // Get match info from active editor
                            let match_info = self
                                .active_editor_mut()
                                .and_then(|editor| editor.get_search_match_info());

                            // Check if we should close modal
                            use crate::ui::modal::SearchAction;
                            if matches!(search_result.action, SearchAction::CloseWithSelection) {
                                // Close modal but keep search state and selection active
                                self.state.close_modal();
                                return Ok(());
                            }

                            // Update match info in modal for other actions
                            if let Some((current, total)) = match_info {
                                if let Some(ActiveModal::Search(search_modal)) =
                                    &mut self.state.active_modal
                                {
                                    search_modal.set_match_info(current, total);
                                }
                            }

                            // Keep modal open for navigation actions
                            return Ok(());
                        }
                    } else if matches!(result, ModalResult::Cancelled) {
                        // Close modal only on cancellation
                        self.state.close_modal();
                        // Also close search state in editor
                        if let Some(editor) = self.active_editor_mut() {
                            editor.close_search();
                        }
                        return Ok(());
                    }
                }

                // Handle ReplaceModal specially - don't close on navigation/replace
                if is_replace {
                    if let ModalResult::Confirmed(value) = &result {
                        if let Some(replace_result) =
                            value.downcast_ref::<crate::ui::modal::ReplaceModalResult>()
                        {
                            // Handle replace action in editor
                            self.handle_replace_action(replace_result)?;

                            // Get match info from active editor
                            let match_info = self
                                .active_editor_mut()
                                .and_then(|editor| editor.get_search_match_info());

                            // Check if we should close modal
                            use crate::ui::modal::ReplaceAction;
                            if matches!(replace_result.action, ReplaceAction::ReplaceAll) {
                                // Close modal for ReplaceAll
                                self.state.close_modal();
                                return Ok(());
                            }

                            // Update match info in modal for other actions
                            if let Some((current, total)) = match_info {
                                if let Some(ActiveModal::Replace(replace_modal)) =
                                    &mut self.state.active_modal
                                {
                                    replace_modal.set_match_info(current, total);
                                }
                            }

                            // Keep modal open for navigation and single replace actions
                            return Ok(());
                        }
                    } else if matches!(result, ModalResult::Cancelled) {
                        // Close modal only on cancellation
                        self.state.close_modal();
                        // Also close search state in editor
                        if let Some(editor) = self.active_editor_mut() {
                            editor.close_search();
                        }
                        return Ok(());
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

    /// Handle mouse event in modal window
    pub(super) fn handle_modal_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        modal_area: ratatui::layout::Rect,
    ) -> Result<()> {
        // Get mutable reference to active modal window
        if let Some(modal) = self.state.get_active_modal_mut() {
            // Handle event in corresponding modal window
            let modal_result = match modal {
                ActiveModal::Confirm(m) => m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Input(m) => m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Select(m) => m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Overwrite(m) => m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Conflict(m) => m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Info(m) => m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::RenamePattern(m) => {
                    m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                        ModalResult::Confirmed(value) => {
                            ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                        }
                        ModalResult::Cancelled => ModalResult::Cancelled,
                    })
                }
                ActiveModal::EditableSelect(m) => {
                    m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                        ModalResult::Confirmed(value) => {
                            ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                        }
                        ModalResult::Cancelled => ModalResult::Cancelled,
                    })
                }
                ActiveModal::Search(m) => m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
                ActiveModal::Replace(m) => m.handle_mouse(mouse, modal_area)?.map(|r| match r {
                    ModalResult::Confirmed(value) => {
                        ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
                    }
                    ModalResult::Cancelled => ModalResult::Cancelled,
                }),
            };

            // If modal window returned result, handle it
            if let Some(result) = modal_result {
                // Check modal type before taking state references
                let is_search = matches!(modal, ActiveModal::Search(_));
                let is_replace = matches!(modal, ActiveModal::Replace(_));

                // Handle SearchModal specially - don't close on navigation
                if is_search {
                    if let ModalResult::Confirmed(value) = &result {
                        if let Some(search_result) =
                            value.downcast_ref::<crate::ui::modal::SearchModalResult>()
                        {
                            // Handle search action in editor
                            self.handle_search_action(search_result)?;

                            // Check if we should close modal
                            use crate::ui::modal::SearchAction;
                            if matches!(search_result.action, SearchAction::CloseWithSelection) {
                                // Close modal but keep search state and selection active
                                self.state.close_modal();
                                return Ok(());
                            }

                            // Get match info from active editor
                            let match_info = self
                                .active_editor_mut()
                                .and_then(|editor| editor.get_search_match_info());

                            // Update match info in modal for other actions
                            if let Some((current, total)) = match_info {
                                if let Some(ActiveModal::Search(search_modal)) =
                                    &mut self.state.active_modal
                                {
                                    search_modal.set_match_info(current, total);
                                }
                            }

                            // Keep modal open for navigation actions
                            return Ok(());
                        }
                    } else if matches!(result, ModalResult::Cancelled) {
                        // Close modal only on cancellation
                        self.state.close_modal();
                        // Also close search state in editor
                        if let Some(editor) = self.active_editor_mut() {
                            editor.close_search();
                        }
                        return Ok(());
                    }
                }

                // Handle ReplaceModal specially - don't close on navigation/replace
                if is_replace {
                    if let ModalResult::Confirmed(value) = &result {
                        if let Some(replace_result) =
                            value.downcast_ref::<crate::ui::modal::ReplaceModalResult>()
                        {
                            // Handle replace action in editor
                            self.handle_replace_action(replace_result)?;

                            // Get match info from active editor
                            let match_info = self
                                .active_editor_mut()
                                .and_then(|editor| editor.get_search_match_info());

                            // Check if we should close modal
                            use crate::ui::modal::ReplaceAction;
                            if matches!(replace_result.action, ReplaceAction::ReplaceAll) {
                                // Close modal for ReplaceAll
                                self.state.close_modal();
                                return Ok(());
                            }

                            // Update match info in modal for other actions
                            if let Some((current, total)) = match_info {
                                if let Some(ActiveModal::Replace(replace_modal)) =
                                    &mut self.state.active_modal
                                {
                                    replace_modal.set_match_info(current, total);
                                }
                            }

                            // Keep modal open for navigation and single replace actions
                            return Ok(());
                        }
                    } else if matches!(result, ModalResult::Cancelled) {
                        // Close modal only on cancellation
                        self.state.close_modal();
                        // Also close search state in editor
                        if let Some(editor) = self.active_editor_mut() {
                            editor.close_search();
                        }
                        return Ok(());
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
                    // ReplaceModal is handled entirely through handle_replace_action
                    // called from handle_modal_key/handle_modal_mouse (lines 183-233, 383-434).
                    // No additional processing needed here, similar to how SearchModal works.
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
        if let Some(query) = value.downcast_ref::<String>() {
            // Start search in active editor (case insensitive by default)
            if let Some(editor) = self.active_editor_mut() {
                editor.start_search(query.clone(), false);
            }
        }
        Ok(())
    }

    /// Handle replace action from ReplaceModal
    fn handle_replace_action(
        &mut self,
        replace_result: &crate::ui::modal::ReplaceModalResult,
    ) -> Result<()> {
        use crate::ui::modal::ReplaceAction;

        // Get active editor
        if let Some(editor) = self.active_editor_mut() {
            match replace_result.action {
                ReplaceAction::Search => {
                    // Perform new search/replace (or update existing)
                    editor.start_replace(
                        replace_result.find_query.clone(),
                        replace_result.replace_with.clone(),
                        false,
                    );
                }
                ReplaceAction::Next => {
                    // Update only replace_with value without rebuilding search
                    editor.update_replace_with(replace_result.replace_with.clone());
                    // Navigate to next match
                    editor.search_next();
                }
                ReplaceAction::Previous => {
                    // Update only replace_with value without rebuilding search
                    editor.update_replace_with(replace_result.replace_with.clone());
                    // Navigate to previous match
                    editor.search_prev();
                }
                ReplaceAction::Replace => {
                    // Update only replace_with value without rebuilding search
                    // This preserves the current_match index for sequential replacement
                    editor.update_replace_with(replace_result.replace_with.clone());
                    // Replace current match and position cursor on next match
                    editor.replace_current()?;
                    // Don't call search_next() - replace_current() already positions cursor correctly
                }
                ReplaceAction::ReplaceAll => {
                    // Update search state with latest values from modal before replacing all
                    editor.start_replace(
                        replace_result.find_query.clone(),
                        replace_result.replace_with.clone(),
                        false,
                    );
                    // Replace all matches (now uses updated replace_with)
                    editor.replace_all()?;
                }
            }
        }
        Ok(())
    }

    /// Handle search action from SearchModal
    fn handle_search_action(
        &mut self,
        search_result: &crate::ui::modal::SearchModalResult,
    ) -> Result<()> {
        use crate::ui::modal::SearchAction;

        // Get active editor
        if let Some(editor) = self.active_editor_mut() {
            match search_result.action {
                SearchAction::Search => {
                    // Perform new search (or update existing)
                    editor.start_search(search_result.query.clone(), false);
                }
                SearchAction::Next => {
                    // Navigate to next match
                    editor.search_next();
                }
                SearchAction::Previous => {
                    // Navigate to previous match
                    editor.search_prev();
                }
                SearchAction::CloseWithSelection => {
                    // Just ensure search is active (will be handled by modal close logic)
                    // Selection is already set by editor methods
                }
            }
        }
        Ok(())
    }
}
