//! Modal window handling for the application.

use anyhow::Result;

use super::App;
use crate::state::ActiveModal;
use termide_modal::{
    Modal, ModalResult, ReplaceAction, ReplaceModalResult, SearchAction, SearchModalResult,
};
use termide_ui::path_utils;

/// Helper to convert typed ModalResult to Box<dyn Any>
fn box_modal_result<T: 'static>(result: ModalResult<T>) -> ModalResult<Box<dyn std::any::Any>> {
    match result {
        ModalResult::Confirmed(value) => {
            ModalResult::Confirmed(Box::new(value) as Box<dyn std::any::Any>)
        }
        ModalResult::Cancelled => ModalResult::Cancelled,
    }
}

/// Result of processing search/replace modal
enum SearchReplaceResult {
    /// Keep modal open (navigation action)
    KeepOpen,
    /// Close modal
    Close,
    /// Modal cancelled - close and clear search
    Cancelled,
    /// Not a search/replace modal
    NotApplicable,
}

impl App {
    /// Handle keyboard event in modal window
    pub(super) fn handle_modal_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Get mutable reference to active modal window
        if let Some(modal) = self.state.get_active_modal_mut() {
            // Handle event in corresponding modal window
            let modal_result = match modal {
                ActiveModal::Confirm(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::Input(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::Select(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::Overwrite(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::Conflict(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::Info(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::RenamePattern(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::EditableSelect(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::Search(m) => m.handle_key(key)?.map(box_modal_result),
                ActiveModal::Replace(m) => m.handle_key(key)?.map(box_modal_result),
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
                        if let termide_state::PendingAction::RenameWithPattern {
                            operation, ..
                        } = action
                        {
                            use termide_modal::ConflictModal;

                            if let Some(source) = operation.current_source() {
                                let final_dest = path_utils::resolve_batch_destination_path(
                                    source,
                                    &operation.destination,
                                    operation.sources.len() == 1,
                                );

                                let remaining_items = operation
                                    .sources
                                    .len()
                                    .saturating_sub(operation.current_index + 1);
                                let modal =
                                    ConflictModal::new(source, &final_dest, remaining_items);
                                self.state.pending_action =
                                    Some(termide_state::PendingAction::ContinueBatchOperation {
                                        operation,
                                    });
                                self.state.active_modal =
                                    Some(ActiveModal::Conflict(Box::new(modal)));
                                return Ok(());
                            }
                        }
                    }
                }

                // Handle search/replace modals with shared helper
                if self
                    .handle_search_replace_modal(is_search, is_replace, &result)
                    .is_some()
                {
                    return Ok(());
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

                // Handle search/replace modals with shared helper
                if self
                    .handle_search_replace_modal(is_search, is_replace, &result)
                    .is_some()
                {
                    return Ok(());
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
        use termide_state::PendingAction;

        if let Some(action) = self.state.take_pending_action() {
            match action {
                PendingAction::CreateFile {
                    panel_index,
                    directory,
                } => {
                    self.handle_create_file(panel_index, directory, value)?;
                }
                PendingAction::CreateDirectory {
                    panel_index,
                    directory,
                } => {
                    self.handle_create_directory(panel_index, directory, value)?;
                }
                PendingAction::DeletePath { panel_index, paths } => {
                    self.handle_delete_path(panel_index, paths, value)?;
                }
                PendingAction::SaveFileAs {
                    panel_index,
                    directory,
                } => {
                    self.handle_save_file_as(panel_index, directory, value)?;
                }
                PendingAction::ClosePanel { panel_index } => {
                    self.handle_close_panel(panel_index, value)?;
                }
                PendingAction::CloseEditorWithSave { panel_index } => {
                    self.handle_close_editor_with_save(panel_index, value)?;
                }
                PendingAction::CloseEditorExternal { panel_index } => {
                    self.handle_close_editor_external(panel_index, value)?;
                }
                PendingAction::CloseEditorConflict { panel_index } => {
                    self.handle_close_editor_conflict(panel_index, value)?;
                }
                PendingAction::OverwriteDecision {
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
                PendingAction::CopyPath {
                    panel_index,
                    sources,
                    target_directory,
                } => {
                    self.handle_copy_path(panel_index, sources, target_directory, value)?;
                }
                PendingAction::MovePath {
                    panel_index,
                    sources,
                    target_directory,
                } => {
                    self.handle_move_path(panel_index, sources, target_directory, value)?;
                }
                PendingAction::BatchFileOperation { operation } => {
                    self.process_batch_operation(operation);
                }
                PendingAction::ContinueBatchOperation { operation } => {
                    self.handle_continue_batch_operation(operation, value)?;
                }
                PendingAction::RenameWithPattern {
                    operation,
                    original_name,
                } => {
                    self.handle_rename_with_pattern(operation, original_name, value)?;
                }
                PendingAction::Search => {
                    self.handle_search(value)?;
                }
                PendingAction::Replace => {
                    // ReplaceModal is handled entirely through handle_replace_action
                    // called from handle_modal_key/handle_modal_mouse (lines 183-233, 383-434).
                    // No additional processing needed here, similar to how SearchModal works.
                }
                PendingAction::QuitApplication => {
                    // User confirmed quit - exit application
                    self.state.quit();
                }
                // Navigation actions are handled in key_handler, should not get here
                PendingAction::NextPanel | PendingAction::PrevPanel => {}
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
    fn handle_replace_action(&mut self, replace_result: &ReplaceModalResult) -> Result<()> {
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
    fn handle_search_action(&mut self, search_result: &SearchModalResult) -> Result<()> {
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

    /// Process search modal result and determine what to do
    fn process_search_modal_result(
        &mut self,
        result: &ModalResult<Box<dyn std::any::Any>>,
    ) -> SearchReplaceResult {
        if let ModalResult::Confirmed(value) = result {
            if let Some(search_result) = value.downcast_ref::<SearchModalResult>() {
                // Handle search action in editor
                if self.handle_search_action(search_result).is_err() {
                    return SearchReplaceResult::Close;
                }

                // Get match info from active editor
                let match_info = self
                    .active_editor_mut()
                    .and_then(|editor| editor.get_search_match_info());

                // Check if we should close modal
                if matches!(search_result.action, SearchAction::CloseWithSelection) {
                    return SearchReplaceResult::Close;
                }

                // Update match info in modal for other actions
                if let Some((current, total)) = match_info {
                    if let Some(ActiveModal::Search(search_modal)) = &mut self.state.active_modal {
                        search_modal.set_match_info(current, total);
                    }
                }

                return SearchReplaceResult::KeepOpen;
            }
        } else if matches!(result, ModalResult::Cancelled) {
            return SearchReplaceResult::Cancelled;
        }
        SearchReplaceResult::NotApplicable
    }

    /// Process replace modal result and determine what to do
    fn process_replace_modal_result(
        &mut self,
        result: &ModalResult<Box<dyn std::any::Any>>,
    ) -> SearchReplaceResult {
        if let ModalResult::Confirmed(value) = result {
            if let Some(replace_result) = value.downcast_ref::<ReplaceModalResult>() {
                // Handle replace action in editor
                if self.handle_replace_action(replace_result).is_err() {
                    return SearchReplaceResult::Close;
                }

                // Get match info from active editor
                let match_info = self
                    .active_editor_mut()
                    .and_then(|editor| editor.get_search_match_info());

                // Check if we should close modal
                if matches!(replace_result.action, ReplaceAction::ReplaceAll) {
                    return SearchReplaceResult::Close;
                }

                // Update match info in modal for other actions
                if let Some((current, total)) = match_info {
                    if let Some(ActiveModal::Replace(replace_modal)) = &mut self.state.active_modal
                    {
                        replace_modal.set_match_info(current, total);
                    }
                }

                return SearchReplaceResult::KeepOpen;
            }
        } else if matches!(result, ModalResult::Cancelled) {
            return SearchReplaceResult::Cancelled;
        }
        SearchReplaceResult::NotApplicable
    }

    /// Handle search/replace modal result and return whether to continue processing
    fn handle_search_replace_modal(
        &mut self,
        is_search: bool,
        is_replace: bool,
        result: &ModalResult<Box<dyn std::any::Any>>,
    ) -> Option<()> {
        if is_search {
            match self.process_search_modal_result(result) {
                SearchReplaceResult::KeepOpen => return Some(()),
                SearchReplaceResult::Close => {
                    self.state.close_modal();
                    return Some(());
                }
                SearchReplaceResult::Cancelled => {
                    self.state.close_modal();
                    if let Some(editor) = self.active_editor_mut() {
                        editor.close_search();
                    }
                    return Some(());
                }
                SearchReplaceResult::NotApplicable => {}
            }
        }

        if is_replace {
            match self.process_replace_modal_result(result) {
                SearchReplaceResult::KeepOpen => return Some(()),
                SearchReplaceResult::Close => {
                    self.state.close_modal();
                    return Some(());
                }
                SearchReplaceResult::Cancelled => {
                    self.state.close_modal();
                    if let Some(editor) = self.active_editor_mut() {
                        editor.close_search();
                    }
                    return Some(());
                }
                SearchReplaceResult::NotApplicable => {}
            }
        }

        None // Continue with normal modal handling
    }
}
