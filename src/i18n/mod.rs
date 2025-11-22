use std::sync::OnceLock;

pub mod en;
pub mod ru;

/// Global translation instance
static TRANSLATION: OnceLock<Box<dyn Translation>> = OnceLock::new();

/// Current language code
static CURRENT_LANGUAGE: OnceLock<String> = OnceLock::new();

/// Translation trait for all user-facing strings
pub trait Translation: Send + Sync {
    // File Manager operations
    fn fm_copy_files(&self) -> &str;
    fn fm_cut_files(&self) -> &str;
    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String;
    fn fm_delete_confirm(&self, count: usize) -> String;
    fn fm_rename_prompt(&self, old_name: &str) -> String;
    fn fm_create_file_prompt(&self) -> &str;
    fn fm_create_dir_prompt(&self) -> &str;
    fn fm_copy_prompt(&self, name: &str) -> String;
    fn fm_move_prompt(&self, name: &str) -> String;
    fn fm_search_prompt(&self) -> &str;
    fn fm_no_results(&self) -> &str;
    fn fm_operation_cancelled(&self) -> &str;

    // Modal buttons
    fn modal_yes(&self) -> &str;
    fn modal_no(&self) -> &str;
    fn modal_ok(&self) -> &str;
    fn modal_cancel(&self) -> &str;

    // Panel titles
    fn panel_file_manager(&self) -> &str;
    fn panel_editor(&self, filename: &str) -> String;
    fn panel_terminal(&self) -> &str;
    fn panel_welcome(&self) -> &str;

    // Editor
    fn editor_close_unsaved(&self) -> &str;
    fn editor_close_unsaved_question(&self) -> &str;
    fn editor_save_and_close(&self) -> &str;
    fn editor_close_without_saving(&self) -> &str;
    fn editor_cancel(&self) -> &str;
    fn editor_save_error(&self, error: &str) -> String;
    fn editor_saved(&self, path: &str) -> String;
    fn editor_file_opened(&self, filename: &str) -> String;
    fn editor_search_title(&self) -> &str;
    fn editor_search_prompt(&self) -> &str;
    fn editor_replace_title(&self) -> &str;
    fn editor_replace_prompt(&self) -> &str;
    fn editor_replace_with_prompt(&self) -> &str;
    fn editor_search_match_info(&self, current: usize, total: usize) -> String;
    fn editor_search_no_matches(&self) -> &str;

    // Terminal
    fn terminal_exit_confirm(&self) -> &str;
    fn terminal_exited(&self, code: i32) -> String;

    // Git status
    fn git_detected(&self) -> &str;
    fn git_not_found(&self) -> &str;

    // Application quit
    fn app_quit_confirm(&self) -> &str;

    // Errors
    fn error_operation_failed(&self, error: &str) -> String;
    fn error_file_exists(&self, path: &str) -> String;
    fn error_invalid_path(&self) -> &str;
    fn error_source_eq_dest(&self) -> &str;
    fn error_dest_is_subdir(&self) -> &str;

    // Help modal
    fn help_title(&self) -> &str;
    fn help_app_title(&self) -> &str;
    fn help_version(&self) -> &str;
    fn help_global_keys(&self) -> &str;
    fn help_file_manager_keys(&self) -> &str;
    fn help_editor_keys(&self) -> &str;
    fn help_terminal_keys(&self) -> &str;
    fn help_git_integration(&self) -> &str;
    fn help_clipboard_operations(&self) -> &str;
    fn help_close_hint(&self) -> &str;

    // Help key descriptions - Global
    fn help_desc_menu(&self) -> &str;
    fn help_desc_panel_menu(&self) -> &str;
    fn help_desc_quit(&self) -> &str;
    fn help_desc_help(&self) -> &str;
    fn help_desc_switch_panel(&self) -> &str;
    fn help_desc_close_panel(&self) -> &str;

    // Help key descriptions - File Manager
    fn help_desc_navigate(&self) -> &str;
    fn help_desc_select(&self) -> &str;
    fn help_desc_select_all(&self) -> &str;
    fn help_desc_open_file(&self) -> &str;
    fn help_desc_open_editor(&self) -> &str;
    fn help_desc_new_terminal(&self) -> &str;
    fn help_desc_parent_dir(&self) -> &str;
    fn help_desc_home(&self) -> &str;
    fn help_desc_end(&self) -> &str;
    fn help_desc_page_scroll(&self) -> &str;
    fn help_desc_create_file(&self) -> &str;
    fn help_desc_create_dir(&self) -> &str;
    fn help_desc_rename(&self) -> &str;
    fn help_desc_copy(&self) -> &str;
    fn help_desc_move(&self) -> &str;
    fn help_desc_delete(&self) -> &str;
    fn help_desc_search(&self) -> &str;
    fn help_desc_fm_copy_clipboard(&self) -> &str;
    fn help_desc_fm_cut_clipboard(&self) -> &str;
    fn help_desc_fm_paste_clipboard(&self) -> &str;

    // Help key descriptions - Editor
    fn help_desc_save(&self) -> &str;
    fn help_desc_copy_system(&self) -> &str;
    fn help_desc_paste_system(&self) -> &str;
    fn help_desc_cut_system(&self) -> &str;
    fn help_desc_paste_ctrl_y(&self) -> &str;
    fn help_desc_undo(&self) -> &str;
    fn help_desc_redo(&self) -> &str;

    // Help key descriptions - Git colors
    fn help_desc_git_ignored(&self) -> &str;
    fn help_desc_git_modified(&self) -> &str;
    fn help_desc_git_added(&self) -> &str;
    fn help_desc_git_deleted(&self) -> &str;

    // File operation status messages
    fn status_file_created(&self, name: &str) -> String;
    fn status_error_create_file(&self, error: &str) -> String;
    fn status_dir_created(&self, name: &str) -> String;
    fn status_error_create_dir(&self, error: &str) -> String;
    fn status_item_deleted(&self) -> &str;
    fn status_error_delete(&self) -> &str;
    fn status_items_deleted(&self, count: usize) -> String;
    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String;
    fn status_file_saved(&self, name: &str) -> String;
    fn status_error_save(&self, error: &str) -> String;
    fn status_error_open_file(&self, name: &str, error: &str) -> String;
    fn status_item_actioned(&self, name: &str, action: &str) -> String;
    fn status_error_action(&self, action: &str, error: &str) -> String;
    fn status_operation_skipped(&self, name: &str) -> String;

    // Action words
    fn action_copied(&self) -> &str;
    fn action_moved(&self) -> &str;
    fn action_copying(&self) -> &str;
    fn action_moving(&self) -> &str;

    // Modal titles and prompts for copy/move
    fn modal_copy_title(&self) -> &str;
    fn modal_move_title(&self) -> &str;
    fn modal_save_as_title(&self) -> &str;
    fn modal_enter_filename(&self) -> &str;
    fn modal_copy_single_prompt(&self, name: &str) -> String;
    fn modal_copy_multiple_prompt(&self, count: usize) -> String;
    fn modal_move_single_prompt(&self, name: &str) -> String;
    fn modal_move_multiple_prompt(&self, count: usize) -> String;

    // Batch operation results
    fn batch_result_file_copied(&self) -> &str;
    fn batch_result_file_moved(&self) -> &str;
    fn batch_result_error_copy(&self) -> &str;
    fn batch_result_error_move(&self) -> &str;
    fn batch_result_copied(&self) -> &str;
    fn batch_result_moved(&self) -> &str;
    fn batch_result_skipped_fmt(&self, count: usize) -> String;
    fn batch_result_errors_fmt(&self, count: usize) -> String;

    // Menu items
    fn menu_files(&self) -> &str;
    fn menu_terminal(&self) -> &str;
    fn menu_editor(&self) -> &str;
    fn menu_debug(&self) -> &str;
    fn menu_preferences(&self) -> &str;
    fn menu_help(&self) -> &str;
    fn menu_quit(&self) -> &str;

    // Menu hints
    fn menu_navigate_hint(&self) -> &str;
    fn menu_open_hint(&self) -> &str;

    // Status bar labels
    fn status_dir(&self) -> &str;
    fn status_file(&self) -> &str;
    fn status_mod(&self) -> &str;
    fn status_owner(&self) -> &str;
    fn status_selected(&self) -> &str;
    fn status_pos(&self) -> &str;
    fn status_tab(&self) -> &str;
    fn status_plain_text(&self) -> &str;
    fn status_readonly(&self) -> &str;
    fn status_cwd(&self) -> &str;
    fn status_shell(&self) -> &str;
    fn status_terminal(&self) -> &str;
    fn status_layout(&self) -> &str;
    fn status_panel(&self) -> &str;

    // UI elements
    fn ui_yes(&self) -> &str;
    fn ui_no(&self) -> &str;
    fn ui_enter_confirm(&self) -> &str;
    fn ui_esc_cancel(&self) -> &str;
    fn ui_hint_separator(&self) -> &str; // " | "

    // File size units
    fn size_bytes(&self) -> &str;
    fn size_kilobytes(&self) -> &str;
    fn size_megabytes(&self) -> &str;
    fn size_gigabytes(&self) -> &str;
    fn size_terabytes(&self) -> &str;

    // File info modal
    fn file_info_title(&self) -> &str;
    fn file_info_title_file(&self, name: &str) -> String;
    fn file_info_title_directory(&self, name: &str) -> String;
    fn file_info_title_symlink(&self, name: &str) -> String;
    fn file_info_name(&self) -> &str;
    fn file_info_path(&self) -> &str;
    fn file_info_type(&self) -> &str;
    fn file_info_size(&self) -> &str;
    fn file_info_owner(&self) -> &str;
    fn file_info_group(&self) -> &str;
    fn file_info_created(&self) -> &str;
    fn file_info_modified(&self) -> &str;
    fn file_info_calculating(&self) -> &str;
    fn file_info_press_key(&self) -> &str;

    // File types
    fn file_type_directory(&self) -> &str;
    fn file_type_file(&self) -> &str;
    fn file_type_symlink(&self) -> &str;
}

/// Initialize translation system based on environment variables
pub fn init() {
    init_with_language("auto");
}

/// Initialize translation system with specified language
/// If lang is "auto", detect from environment variables
pub fn init_with_language(lang: &str) {
    let detected = if lang == "auto" || lang.is_empty() {
        detect_language()
    } else {
        lang.to_string()
    };

    let translation: Box<dyn Translation> = match detected.as_str() {
        "ru" => Box::new(ru::Russian),
        _ => Box::new(en::English),
    };

    let _ = TRANSLATION.set(translation);
    let _ = CURRENT_LANGUAGE.set(detected);
}

/// Detect language from environment variables
fn detect_language() -> String {
    // Check TERMIDE_LANG first (app-specific)
    if let Ok(lang) = std::env::var("TERMIDE_LANG") {
        return normalize_lang(&lang);
    }

    // Check LANG
    if let Ok(lang) = std::env::var("LANG") {
        return normalize_lang(&lang);
    }

    // Check LC_ALL
    if let Ok(lang) = std::env::var("LC_ALL") {
        return normalize_lang(&lang);
    }

    // Default to English
    "en".to_string()
}

/// Normalize language string (e.g., "ru_RU.UTF-8" -> "ru")
fn normalize_lang(lang: &str) -> String {
    lang.split('_')
        .next()
        .unwrap_or("en")
        .split('.')
        .next()
        .unwrap_or("en")
        .to_lowercase()
}

/// Get the current translation
pub fn t() -> &'static dyn Translation {
    TRANSLATION.get().map(|b| b.as_ref()).unwrap_or(&en::English)
}

/// Get the current language code ("en", "ru", etc.)
pub fn current_language() -> &'static str {
    CURRENT_LANGUAGE.get().map(|s| s.as_str()).unwrap_or("en")
}
