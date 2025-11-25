use super::Translation;

/// English translation (default/fallback)
pub struct English;

impl Translation for English {
    // File Manager operations
    fn fm_copy_files(&self) -> &str {
        "Files copied to clipboard"
    }

    fn fm_cut_files(&self) -> &str {
        "Files cut to clipboard"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        format!(
            "{} {} file{} to:\n{}",
            mode,
            count,
            if count == 1 { "" } else { "s" },
            dest
        )
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!(
            "Delete {} file{}?",
            count,
            if count == 1 { "" } else { "s" }
        )
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("Rename '{}' to:", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "Enter file name:"
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "Enter directory name:"
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("Copy '{}' to:", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("Move '{}' to:", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "Search:"
    }

    fn fm_no_results(&self) -> &str {
        "No matching files found"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "Operation cancelled"
    }

    // Modal buttons
    fn modal_yes(&self) -> &str {
        "Yes"
    }

    fn modal_no(&self) -> &str {
        "No"
    }

    fn modal_ok(&self) -> &str {
        "OK"
    }

    fn modal_cancel(&self) -> &str {
        "Cancel"
    }

    // Panel titles
    fn panel_file_manager(&self) -> &str {
        "File Manager"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("Editor: {}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "Terminal"
    }

    fn panel_welcome(&self) -> &str {
        "Welcome"
    }

    // Editor
    fn editor_close_unsaved(&self) -> &str {
        "Close Editor"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "File has unsaved changes. What to do?"
    }

    fn editor_save_and_close(&self) -> &str {
        "Save and close"
    }

    fn editor_close_without_saving(&self) -> &str {
        "Close without saving"
    }

    fn editor_cancel(&self) -> &str {
        "Cancel"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("Failed to save file: {}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("File saved: {}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("File '{}' opened", filename)
    }

    fn editor_search_title(&self) -> &str {
        "Search"
    }

    fn editor_search_prompt(&self) -> &str {
        "Enter search query:"
    }

    fn editor_replace_title(&self) -> &str {
        "Replace"
    }

    fn editor_replace_prompt(&self) -> &str {
        "Search for:"
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "Replace with:"
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("Match {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "No matches"
    }

    // Terminal
    fn terminal_exit_confirm(&self) -> &str {
        "Process is still running. Close terminal?"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("Process exited with code {}", code)
    }

    // Git status
    fn git_detected(&self) -> &str {
        "Git detected and available"
    }

    fn git_not_found(&self) -> &str {
        "Git not found - git integration disabled"
    }

    fn app_quit_confirm(&self) -> &str {
        "There are unsaved changes. Quit anyway?"
    }

    // Errors
    fn error_operation_failed(&self, error: &str) -> String {
        format!("Operation failed: {}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("File or directory already exists: {}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "Invalid path"
    }

    fn error_source_eq_dest(&self) -> &str {
        "Source and destination are the same"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "Destination is a subdirectory of source"
    }

    // Help modal
    fn help_title(&self) -> &str {
        "Help"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - Help"
    }

    fn help_version(&self) -> &str {
        "v0.1.0"
    }

    fn help_global_keys(&self) -> &str {
        "GLOBAL HOTKEYS"
    }

    fn help_file_manager_keys(&self) -> &str {
        "FILE MANAGER"
    }

    fn help_editor_keys(&self) -> &str {
        "TEXT EDITOR"
    }

    fn help_terminal_keys(&self) -> &str {
        "TERMINAL"
    }

    fn help_git_integration(&self) -> &str {
        "GIT INTEGRATION"
    }

    fn help_clipboard_operations(&self) -> &str {
        "CLIPBOARD OPERATIONS"
    }

    fn help_close_hint(&self) -> &str {
        "Press Esc or Ctrl+H to close this window"
    }

    // Help key descriptions - Global
    fn help_desc_menu(&self) -> &str {
        "Open/close menu"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "Panel menu"
    }

    fn help_desc_quit(&self) -> &str {
        "Quit application"
    }

    fn help_desc_help(&self) -> &str {
        "Show help"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "Switch between panels"
    }

    fn help_desc_close_panel(&self) -> &str {
        "Close active panel"
    }

    // Help key descriptions - File Manager
    fn help_desc_navigate(&self) -> &str {
        "Navigate through list"
    }

    fn help_desc_select(&self) -> &str {
        "Toggle selection"
    }

    fn help_desc_select_all(&self) -> &str {
        "Select all files"
    }

    fn help_desc_open_file(&self) -> &str {
        "Open directory or file in editor"
    }

    fn help_desc_open_editor(&self) -> &str {
        "Open file in editor"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "Open new terminal in current directory"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "Go to parent directory"
    }

    fn help_desc_home(&self) -> &str {
        "Go to start of list"
    }

    fn help_desc_end(&self) -> &str {
        "Go to end of list"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "Scroll by pages"
    }

    fn help_desc_create_file(&self) -> &str {
        "Create new file"
    }

    fn help_desc_create_dir(&self) -> &str {
        "Create new directory"
    }

    fn help_desc_rename(&self) -> &str {
        "Rename file/directory"
    }

    fn help_desc_copy(&self) -> &str {
        "Copy file/directory"
    }

    fn help_desc_move(&self) -> &str {
        "Move file/directory"
    }

    fn help_desc_delete(&self) -> &str {
        "Delete selected file/directory"
    }

    fn help_desc_search(&self) -> &str {
        "Search files"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "Copy selected files to file clipboard"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "Cut selected files to file clipboard"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "Paste files from file clipboard"
    }

    // Help key descriptions - Editor
    fn help_desc_save(&self) -> &str {
        "Save file"
    }

    fn help_desc_copy_system(&self) -> &str {
        "Copy to system clipboard"
    }

    fn help_desc_paste_system(&self) -> &str {
        "Paste from system clipboard"
    }

    fn help_desc_cut_system(&self) -> &str {
        "Cut to system clipboard"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "Paste from system clipboard"
    }

    fn help_desc_undo(&self) -> &str {
        "Undo"
    }

    fn help_desc_redo(&self) -> &str {
        "Redo"
    }

    // Help key descriptions - Git colors
    fn help_desc_git_ignored(&self) -> &str {
        "Dark gray - ignored by git"
    }

    fn help_desc_git_modified(&self) -> &str {
        "Yellow - modified"
    }

    fn help_desc_git_added(&self) -> &str {
        "Green - new/added"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "Faded red - deleted (read-only)"
    }

    // File operation status messages
    fn status_file_created(&self, name: &str) -> String {
        format!("File '{}' created", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("Error creating file: {}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("Directory '{}' created", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("Error creating directory: {}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "Item deleted"
    }

    fn status_error_delete(&self) -> &str {
        "Delete error"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("Deleted {} items", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("Deleted: {}, errors: {}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("File '{}' saved", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("Save error: {}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("Error opening '{}': {}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("Error {}: {}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("Operation '{}' skipped", name)
    }

    // Action words
    fn action_copied(&self) -> &str {
        "copied"
    }

    fn action_moved(&self) -> &str {
        "moved"
    }

    fn action_copying(&self) -> &str {
        "copying"
    }

    fn action_moving(&self) -> &str {
        "moving"
    }

    // Modal titles and prompts for copy/move
    fn modal_copy_single_title(&self, name: &str) -> String {
        format!("Copy '{}'", name)
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        format!("Copy {} elements", count)
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        format!("Move '{}'", name)
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        format!("Move {} elements", count)
    }

    fn modal_create_file_title(&self) -> &str {
        "Create File"
    }

    fn modal_create_dir_title(&self) -> &str {
        "Create Directory"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("Delete '{}'", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("Delete {} elements", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "Save As"
    }

    fn modal_enter_filename(&self) -> &str {
        "Enter file name:"
    }

    fn modal_copy_single_prompt(&self, _name: &str) -> String {
        String::new()
    }

    fn modal_copy_multiple_prompt(&self, _count: usize) -> String {
        String::new()
    }

    fn modal_move_single_prompt(&self, _name: &str) -> String {
        String::new()
    }

    fn modal_move_multiple_prompt(&self, _count: usize) -> String {
        String::new()
    }

    // Batch operation results
    fn batch_result_file_copied(&self) -> &str {
        "copied"
    }

    fn batch_result_file_moved(&self) -> &str {
        "moved"
    }

    fn batch_result_error_copy(&self) -> &str {
        "Error copy"
    }

    fn batch_result_error_move(&self) -> &str {
        "Error move"
    }

    fn batch_result_copied(&self) -> &str {
        "Copied"
    }

    fn batch_result_moved(&self) -> &str {
        "Moved"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("skipped: {}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("errors: {}", count)
    }

    // Menu items
    fn menu_files(&self) -> &str {
        "Files"
    }

    fn menu_terminal(&self) -> &str {
        "Terminal"
    }

    fn menu_editor(&self) -> &str {
        "Editor"
    }

    fn menu_debug(&self) -> &str {
        "Debug"
    }

    fn menu_preferences(&self) -> &str {
        "Preferences"
    }

    fn menu_help(&self) -> &str {
        "Help"
    }

    fn menu_quit(&self) -> &str {
        "Quit"
    }

    // Menu hints
    fn menu_navigate_hint(&self) -> &str {
        "←→ Navigate | Enter Select | Esc Close"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M Menu"
    }

    // Status bar labels
    fn status_dir(&self) -> &str {
        "Dir:"
    }

    fn status_file(&self) -> &str {
        "File:"
    }

    fn status_mod(&self) -> &str {
        "Mod:"
    }

    fn status_owner(&self) -> &str {
        "Owner:"
    }

    fn status_selected(&self) -> &str {
        "Selected:"
    }

    fn status_pos(&self) -> &str {
        "Pos:"
    }

    fn status_tab(&self) -> &str {
        "Tab:"
    }

    fn status_plain_text(&self) -> &str {
        "Plain Text"
    }

    fn status_readonly(&self) -> &str {
        "[RO]"
    }

    fn status_cwd(&self) -> &str {
        "CWD:"
    }

    fn status_shell(&self) -> &str {
        "Shell:"
    }

    fn status_terminal(&self) -> &str {
        "Terminal:"
    }

    fn status_layout(&self) -> &str {
        "Layout:"
    }

    fn status_panel(&self) -> &str {
        "Panel:"
    }

    // UI elements
    fn ui_yes(&self) -> &str {
        "Yes"
    }

    fn ui_no(&self) -> &str {
        "No"
    }

    fn ui_ok(&self) -> &str {
        "OK"
    }

    fn ui_cancel(&self) -> &str {
        "Cancel"
    }

    fn ui_continue(&self) -> &str {
        "Continue"
    }

    fn ui_close(&self) -> &str {
        "Close"
    }

    fn ui_enter_confirm(&self) -> &str {
        "Enter - confirm"
    }

    fn ui_esc_cancel(&self) -> &str {
        "Esc - cancel"
    }

    fn ui_hint_separator(&self) -> &str {
        " | "
    }

    // File size units
    fn size_bytes(&self) -> &str {
        "B"
    }

    fn size_kilobytes(&self) -> &str {
        "KB"
    }

    fn size_megabytes(&self) -> &str {
        "MB"
    }

    fn size_gigabytes(&self) -> &str {
        "GB"
    }

    fn size_terabytes(&self) -> &str {
        "TB"
    }

    // File info modal
    fn file_info_title(&self) -> &str {
        "File Info"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("File info '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("Directory info '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("Symlink info '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "Name"
    }

    fn file_info_path(&self) -> &str {
        "Path"
    }

    fn file_info_type(&self) -> &str {
        "Type"
    }

    fn file_info_size(&self) -> &str {
        "Size"
    }

    fn file_info_owner(&self) -> &str {
        "Owner"
    }

    fn file_info_group(&self) -> &str {
        "Group"
    }

    fn file_info_created(&self) -> &str {
        "Created"
    }

    fn file_info_modified(&self) -> &str {
        "Modified"
    }

    fn file_info_calculating(&self) -> &str {
        "Calculating"
    }

    fn file_info_press_key(&self) -> &str {
        "Press any key to close"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        if count == 0 {
            "no changes to commit".to_string()
        } else if count == 1 {
            "1 change to commit".to_string()
        } else {
            format!("{} changes to commit", count)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        if count == 0 {
            "no commits to push".to_string()
        } else if count == 1 {
            "1 commit to push".to_string()
        } else {
            format!("{} commits to push", count)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        if count == 0 {
            "no commits to pull".to_string()
        } else if count == 1 {
            "1 commit to pull".to_string()
        } else {
            format!("{} commits to pull", count)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "not in git index"
    }

    // File types
    fn file_type_directory(&self) -> &str {
        "Directory"
    }

    fn file_type_file(&self) -> &str {
        "File"
    }

    fn file_type_symlink(&self) -> &str {
        "Symlink"
    }
}
