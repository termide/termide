//! Internationalization support for termide.
//!
//! Provides translation loading and language detection.

// I18n trait methods are prepared for future use
#![allow(dead_code)]

use std::sync::RwLock;

mod detect;
pub mod loader;
pub mod runtime;

pub use detect::detect_language;
pub use loader::{Metadata, PluralRules, TranslationData};

/// Supported languages with their native names (code, native_name).
/// Sorted alphabetically by language code.
pub const SUPPORTED_LANGUAGES: &[(&str, &str)] = &[
    ("bn", "বাংলা"),
    ("de", "Deutsch"),
    ("en", "English"),
    ("es", "Español"),
    ("fr", "Français"),
    ("hi", "हिन्दी"),
    ("id", "Bahasa Indonesia"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("pt", "Português"),
    ("ru", "Русский"),
    ("th", "ไทย"),
    ("tr", "Türkçe"),
    ("vi", "Tiếng Việt"),
    ("zh", "中文"),
];

/// Global translation instance (RwLock for runtime switching).
///
/// Stores a leaked `&'static` reference so that `t()` can return `&'static dyn Translation`
/// without unsafe pointer casts. Old translations are intentionally leaked on language switch
/// (a few KB each, happens rarely).
static TRANSLATION: RwLock<Option<&'static dyn Translation>> = RwLock::new(None);

/// Current language code (RwLock for runtime switching).
static CURRENT_LANGUAGE: RwLock<String> = RwLock::new(String::new());

/// Bumped on every successful language change so cheap, hot-path caches of
/// translated strings (e.g. the menu bar) can detect a switch with a single
/// atomic load instead of allocating to read [`current_language`].
static LANGUAGE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Translation trait for all user-facing strings.
pub trait Translation: Send + Sync {
    // File Manager operations
    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String;
    fn fm_copy_prompt(&self, name: &str) -> String;
    fn fm_move_prompt(&self, name: &str) -> String;
    fn git_operation_cancelled(&self) -> &str;

    // Modal buttons
    fn modal_yes(&self) -> &str;
    fn modal_ok(&self) -> &str;
    // Panel titles
    fn panel_help(&self) -> &str;
    fn panel_journal(&self) -> &str;
    fn panel_operations(&self) -> &str;

    // Operations panel
    fn no_active_operations(&self) -> &str;

    // Editor
    fn editor_close_unsaved(&self) -> &str;
    fn editor_close_unsaved_question(&self) -> &str;
    fn editor_save_and_close(&self) -> &str;
    fn editor_close_without_saving(&self) -> &str;
    fn editor_cancel(&self) -> &str;
    fn editor_close_external(&self) -> &str;
    fn editor_close_external_question(&self) -> &str;
    fn editor_overwrite_disk(&self) -> &str;
    fn editor_keep_disk_close(&self) -> &str;
    fn editor_reload_into_editor(&self) -> &str;
    fn editor_close_conflict(&self) -> &str;
    fn editor_close_conflict_question(&self) -> &str;
    fn editor_reload_from_disk(&self) -> &str;
    fn editor_file_opened(&self, filename: &str) -> String;
    fn editor_search_match_info(&self, current: usize, total: usize) -> String;
    fn editor_search_no_matches(&self) -> &str;
    fn editor_deletion_marker(&self, count: usize) -> String;

    // File manager
    fn fm_goto_title(&self) -> &str;
    fn fm_goto_prompt(&self) -> &str;
    fn connection_cancelled_title(&self) -> &str;
    fn connection_error_title(&self) -> &str;
    fn vfs_reconnect(&self) -> &str;
    fn vfs_open_local(&self) -> &str;
    fn vfs_close_panel(&self) -> &str;
    fn db_reconnect(&self) -> &str;
    fn db_close_panel(&self) -> &str;
    fn connection_timeout_title(&self) -> &str;
    fn connection_timeout_message(&self) -> &str;

    // Terminal
    // Git status
    // Application quit
    fn app_quit_confirm(&self) -> &str;
    fn app_quit_title(&self) -> &str;

    // Errors
    // Help modal
    fn help_global_keys(&self) -> &str;
    fn help_file_manager_keys(&self) -> &str;
    fn help_editor_keys(&self) -> &str;
    fn help_terminal_keys(&self) -> &str;
    // Help key descriptions
    fn help_desc_menu(&self) -> &str;
    fn help_desc_quit(&self) -> &str;
    fn help_desc_help(&self) -> &str;
    fn help_desc_close_panel(&self) -> &str;
    fn help_desc_escape_close(&self) -> &str;
    fn help_desc_select(&self) -> &str;
    fn help_desc_new_terminal(&self) -> &str;
    fn help_desc_home(&self) -> &str;
    fn help_desc_end(&self) -> &str;
    fn help_desc_page_scroll(&self) -> &str;
    fn help_desc_create_file(&self) -> &str;
    fn help_desc_create_dir(&self) -> &str;
    fn help_desc_copy(&self) -> &str;
    fn help_desc_move(&self) -> &str;
    fn help_desc_rename(&self) -> &str;
    // Help section headers
    fn help_section_panels(&self) -> &str;
    fn help_section_git_status(&self) -> &str;
    fn help_section_navigation(&self) -> &str;
    fn help_section_git_diff(&self) -> &str;
    fn help_section_git_log(&self) -> &str;

    // Additional help key descriptions
    fn help_desc_new_file_manager(&self) -> &str;
    fn help_desc_new_editor(&self) -> &str;
    fn help_desc_new_journal(&self) -> &str;
    fn help_desc_open_preferences(&self) -> &str;
    fn help_desc_open_sessions(&self) -> &str;
    fn help_desc_open_git_status(&self) -> &str;
    fn help_desc_open_outline(&self) -> &str;
    fn help_desc_open_diagnostics(&self) -> &str;
    fn help_desc_open_git_log(&self) -> &str;
    fn help_desc_toggle_stack(&self) -> &str;
    fn help_desc_swap_left(&self) -> &str;
    fn help_desc_swap_right(&self) -> &str;
    fn help_desc_panel_action_menu(&self) -> &str;
    // Panel action context menu (short labels)
    fn panel_action_close(&self) -> &str;
    fn panel_action_split(&self) -> &str;
    fn panel_action_merge(&self) -> &str;
    fn panel_action_move_left(&self) -> &str;
    fn panel_action_move_right(&self) -> &str;
    fn panel_action_move_up(&self) -> &str;
    fn panel_action_move_down(&self) -> &str;
    fn help_desc_move_first(&self) -> &str;
    fn help_desc_move_last(&self) -> &str;
    fn help_desc_resize_smaller(&self) -> &str;
    fn help_desc_resize_larger(&self) -> &str;
    fn help_desc_toggle_fullscreen_panel(&self) -> &str;
    fn help_desc_panel_grow_vertical(&self) -> &str;
    fn help_desc_panel_shrink_vertical(&self) -> &str;
    fn help_desc_prev_group(&self) -> &str;
    fn help_desc_next_group(&self) -> &str;
    fn help_desc_prev_panel(&self) -> &str;
    fn help_desc_next_panel(&self) -> &str;
    fn help_desc_goto_panel(&self) -> &str;
    fn help_desc_save_as(&self) -> &str;
    fn help_desc_reload(&self) -> &str;
    fn help_desc_duplicate_line(&self) -> &str;
    fn help_desc_delete_line(&self) -> &str;
    fn help_desc_toggle_comment(&self) -> &str;
    fn help_desc_search_next(&self) -> &str;
    fn help_desc_search_prev(&self) -> &str;
    fn help_desc_replace(&self) -> &str;
    fn help_desc_replace_current(&self) -> &str;
    fn help_desc_replace_all(&self) -> &str;
    // LSP help descriptions
    fn help_desc_trigger_completion(&self) -> &str;
    fn help_desc_show_hover(&self) -> &str;
    fn help_desc_goto_definition(&self) -> &str;
    fn help_desc_find_references(&self) -> &str;
    fn help_desc_rename_symbol(&self) -> &str;
    fn help_desc_code_action(&self) -> &str;

    // LSP rename flow status messages
    fn lsp_rename_no_identifier(&self) -> &str;
    fn lsp_rename_unsaved_file(&self) -> &str;
    fn lsp_rename_no_changes(&self) -> &str;
    fn lsp_rename_result(&self, count: usize) -> String;

    fn help_desc_word_nav(&self) -> &str;
    fn help_desc_paragraph_nav(&self) -> &str;
    fn help_desc_view_file(&self) -> &str;
    fn help_desc_edit_file(&self) -> &str;
    fn help_desc_toggle_hidden(&self) -> &str;
    fn help_desc_open_external(&self) -> &str;
    fn help_desc_delete_generic(&self) -> &str;
    fn help_desc_open_bookmark_add(&self) -> &str;
    fn help_desc_command_palette(&self) -> &str;
    fn help_desc_stage_file(&self) -> &str;
    fn help_desc_unstage_file(&self) -> &str;
    fn help_desc_terminal_copy(&self) -> &str;
    fn help_desc_terminal_paste(&self) -> &str;
    fn help_desc_scroll_up(&self) -> &str;
    fn help_desc_scroll_down(&self) -> &str;
    fn help_desc_scroll_top(&self) -> &str;
    fn help_desc_scroll_bottom(&self) -> &str;

    // Navigation help descriptions (static keys)
    fn help_desc_move_up(&self) -> &str;
    fn help_desc_move_down(&self) -> &str;
    fn help_desc_scroll_half_up(&self) -> &str;

    // Git Diff help descriptions (static keys)
    fn help_desc_toggle_collapse(&self) -> &str;
    fn help_desc_open_file_editor(&self) -> &str;

    // Git Log help descriptions (static keys)
    fn help_desc_view_commit_diff(&self) -> &str;

    // Additional help descriptions (missing entries audit)
    fn help_desc_new_session(&self) -> &str;
    fn help_desc_save(&self) -> &str;
    fn help_desc_undo(&self) -> &str;
    fn help_desc_redo(&self) -> &str;
    fn help_desc_search(&self) -> &str;
    fn help_desc_search_content(&self) -> &str;
    fn help_desc_select_all(&self) -> &str;
    fn help_desc_refresh(&self) -> &str;
    fn help_desc_go_parent(&self) -> &str;
    fn help_desc_go_home_dir(&self) -> &str;
    fn help_desc_switch_directory(&self) -> &str;
    fn help_desc_go_to_path(&self) -> &str;
    fn help_desc_edit_copy(&self) -> &str;
    fn help_desc_edit_cut(&self) -> &str;
    fn help_desc_edit_paste(&self) -> &str;
    fn help_desc_view_diff(&self) -> &str;
    fn help_desc_revert(&self) -> &str;
    fn help_desc_checkout(&self) -> &str;
    fn help_desc_copy_hash(&self) -> &str;
    fn help_desc_scroll_half_down(&self) -> &str;
    fn help_desc_stage_unstage(&self) -> &str;
    fn help_desc_tree_search(&self) -> &str;
    fn help_desc_expand_dir(&self) -> &str;
    fn help_desc_collapse_dir(&self) -> &str;
    fn help_desc_word_select(&self) -> &str;
    fn help_desc_paragraph_select(&self) -> &str;
    fn help_desc_switch_focus(&self) -> &str;
    fn help_desc_open_in_browser(&self) -> &str;

    // Help sections for additional panels
    fn help_section_diagnostics(&self) -> &str;
    fn help_section_operations(&self) -> &str;
    fn help_section_outline(&self) -> &str;
    fn help_section_references(&self) -> &str;
    fn help_section_image(&self) -> &str;
    fn help_section_database(&self) -> &str;
    fn help_desc_db_sort(&self) -> &str;
    fn help_desc_db_filter(&self) -> &str;
    fn help_desc_db_clear_filter(&self) -> &str;
    fn help_desc_db_detail(&self) -> &str;
    fn help_desc_db_copy_cell(&self) -> &str;
    fn help_desc_db_copy_row(&self) -> &str;
    fn help_desc_toggle_filter(&self) -> &str;
    fn help_desc_pause_resume(&self) -> &str;
    fn help_desc_cancel_operation(&self) -> &str;
    fn help_desc_navigate(&self) -> &str;
    fn help_desc_copy_name(&self) -> &str;
    fn help_desc_close_image(&self) -> &str;
    fn help_desc_vim_panel_nav(&self) -> &str;
    fn help_section_viewers(&self) -> &str;
    fn help_desc_viewer_toggle(&self) -> &str;
    fn help_desc_viewer_goto(&self) -> &str;
    fn help_desc_viewer_search(&self) -> &str;
    fn help_desc_viewer_reload(&self) -> &str;
    fn help_desc_viewer_copy(&self) -> &str;
    fn help_desc_viewer_follow(&self) -> &str;
    fn help_desc_viewer_external(&self) -> &str;
    fn help_desc_viewer_history(&self) -> &str;

    // File operation status
    fn status_file_created(&self, name: &str) -> String;
    fn status_dir_created(&self, name: &str) -> String;
    fn status_file_saved(&self, name: &str) -> String;
    fn status_error_save(&self, error: &str) -> String;
    fn status_file_reloaded(&self) -> &str;
    fn status_error_reload(&self, error: &str) -> String;
    fn status_error_open_file(&self, name: &str, error: &str) -> String;
    fn status_opening_external(&self, name: &str) -> String;

    // Action words
    // Modal titles
    fn modal_copy_single_title(&self, name: &str) -> String;
    fn modal_copy_multiple_title(&self, count: usize) -> String;
    fn modal_move_single_title(&self, name: &str) -> String;
    fn modal_move_multiple_title(&self, count: usize) -> String;
    fn modal_create_file_title(&self) -> &str;
    fn modal_create_dir_title(&self) -> &str;
    fn modal_delete_single_title(&self, name: &str) -> String;
    fn modal_delete_multiple_title(&self, count: usize) -> String;
    fn modal_save_as_title(&self) -> &str;
    fn modal_copy_single_prompt(&self, name: &str) -> String;
    fn modal_copy_multiple_prompt(&self, count: usize) -> String;
    fn modal_move_single_prompt(&self, name: &str) -> String;
    fn modal_move_multiple_prompt(&self, count: usize) -> String;

    // Batch results
    fn batch_result_file_copied(&self) -> &str;
    fn batch_result_file_moved(&self) -> &str;
    fn batch_result_error_copy(&self) -> &str;
    fn batch_result_error_move(&self) -> &str;
    fn batch_result_copied(&self) -> &str;
    fn batch_result_moved(&self) -> &str;
    fn batch_result_skipped_fmt(&self, count: usize) -> String;
    fn batch_result_errors_fmt(&self, count: usize) -> String;

    // Menu
    fn menu_sessions(&self) -> &str;
    fn menu_windows(&self) -> &str;
    fn menu_commands(&self) -> &str;
    fn menu_commands_add(&self) -> &str;

    // Command parameters modal
    fn command_params_title(&self) -> &str;
    fn command_params_run(&self) -> &str;
    fn command_params_cancel(&self) -> &str;
    fn command_run_label(&self) -> &str;

    // Command config modal (create/edit)
    fn command_config_label_name(&self) -> &str;
    fn command_config_label_command(&self) -> &str;
    fn command_config_label_group(&self) -> &str;
    fn command_config_label_display_name(&self) -> &str;
    fn command_config_label_mode(&self) -> &str;
    fn command_config_label_hotkey(&self) -> &str;
    fn command_config_label_project(&self) -> &str;
    fn command_config_project_checkbox(&self) -> &str;
    fn command_config_hotkey_hint(&self) -> &str;
    fn command_config_hotkey_invalid(&self) -> &str;
    fn command_config_hotkey_conflict(&self) -> &str;
    fn command_config_button_create(&self) -> &str;
    fn command_config_button_save(&self) -> &str;
    fn command_config_button_edit_file(&self) -> &str;
    fn command_config_button_cancel(&self) -> &str;
    fn command_config_mode_terminal(&self) -> &str;
    fn command_config_mode_background(&self) -> &str;
    fn command_config_mode_report(&self) -> &str;
    fn command_config_group_root(&self) -> &str;

    fn menu_options(&self) -> &str;
    fn menu_quit(&self) -> &str;
    fn menu_bookmarks(&self) -> &str;

    // Diagram (Mermaid / editor "view as diagram") menu + status
    fn menu_copy_diagram(&self) -> &str;
    fn menu_save_diagram_as(&self) -> &str;
    fn menu_view_as_diagram(&self) -> &str;
    fn status_no_diagram_symbols(&self) -> &str;
    fn status_diagram_copied(&self, lines: usize) -> String;

    // Bookmarks submenu
    fn bookmarks_add_bookmark(&self) -> &str;
    fn bookmarks_no_bookmarks(&self) -> &str;
    fn bookmarks_add_title(&self) -> &str;
    fn bookmarks_add_path(&self) -> &str;
    fn bookmarks_add_description(&self) -> &str;
    fn bookmarks_add_group(&self) -> &str;
    fn bookmarks_add_project(&self) -> &str;
    // Tools submenu
    fn tools_files(&self) -> &str;
    fn tools_terminal(&self) -> &str;
    fn tools_editor(&self) -> &str;
    fn tools_git_status(&self) -> &str;
    fn tools_git_log(&self) -> &str;
    fn stash_new(&self) -> &str;
    fn stash_include_untracked(&self) -> &str;
    fn stash_created(&self) -> &str;
    fn stash_changes(&self) -> &str;
    fn stash_files(&self) -> &str;
    fn stash_more(&self) -> &str;
    fn stash_pop(&self) -> &str;
    fn stash_apply(&self) -> &str;
    fn stash_drop(&self) -> &str;
    fn stash_diff(&self) -> &str;
    fn git_stash_button(&self) -> &str;
    fn tools_journal(&self) -> &str;
    fn tools_diagnostics(&self) -> &str;
    fn tools_operations(&self) -> &str;
    fn tools_outline(&self) -> &str;
    fn tools_open(&self) -> &str;
    fn tools_open_prompt(&self) -> &str;

    // Options submenu
    fn options_help(&self) -> &str;

    // Git action buttons
    fn git_action_diff(&self) -> &str;
    fn git_action_revert(&self) -> &str;
    fn git_action_close(&self) -> &str;
    fn git_action_init(&self) -> &str;
    fn git_init_success(&self, path: &str) -> String;
    fn git_action_commit(&self) -> &str;
    fn git_action_push(&self) -> &str;
    fn git_action_pull(&self) -> &str;
    fn git_revert_confirm(&self) -> &str;

    // Git commit modal
    fn git_commit_title(&self, count: usize, repo: &str, branch: &str) -> String;

    // Git file properties modal
    fn git_file_properties_title(&self) -> &str;
    fn git_props_path(&self) -> &str;
    fn git_props_status(&self) -> &str;
    fn git_props_size(&self) -> &str;
    fn git_props_diff(&self) -> &str;
    fn git_props_deleted(&self) -> &str;
    fn git_action_edit(&self) -> &str;
    fn git_status_added(&self) -> String;
    fn git_status_deleted(&self) -> String;
    fn git_status_modified(&self) -> String;
    fn git_status_renamed(&self) -> String;
    fn git_status_untracked(&self) -> String;

    // Git operation progress messages
    fn git_push_in_progress(&self) -> String;
    fn git_pull_in_progress(&self) -> String;
    fn git_fetch_in_progress(&self) -> String;

    // Git operation result messages
    fn git_push_success(&self) -> String;
    fn git_push_failed(&self) -> String;
    fn git_pull_success(&self) -> String;
    fn git_pull_failed(&self) -> String;
    fn git_completed(&self) -> String;
    fn git_operation_timed_out(&self) -> &str;

    // Preferences submenu
    fn preferences_themes(&self) -> &str;
    fn preferences_language(&self) -> &str;
    fn preferences_edit(&self) -> &str;
    fn theme_changed(&self, name: &str) -> String;
    fn language_changed(&self, name: &str) -> String;

    // Settings modal — tabs
    fn settings_tab_general(&self) -> &str;
    fn settings_tab_editor(&self) -> &str;
    fn settings_tab_file_manager(&self) -> &str;
    fn settings_tab_terminal(&self) -> &str;
    fn settings_tab_lsp(&self) -> &str;
    fn settings_tab_logging(&self) -> &str;
    fn settings_tab_vfs(&self) -> &str;
    fn settings_tab_keybindings(&self) -> &str;

    // Settings modal — buttons
    fn settings_btn_apply(&self) -> &str;
    fn settings_btn_reset(&self) -> &str;
    fn settings_btn_cancel(&self) -> &str;
    fn settings_btn_create_project_override(&self) -> &str;
    fn settings_btn_remove_project_override(&self) -> &str;
    fn settings_remove_project_override_title(&self) -> &str;
    fn settings_remove_project_override_message(&self) -> &str;

    // Settings modal — General fields
    fn settings_general_vim_mode(&self) -> &str;
    fn settings_general_theme(&self) -> &str;
    fn settings_general_language(&self) -> &str;
    fn settings_general_icon_mode(&self) -> &str;
    fn settings_general_auto_stack_threshold(&self) -> &str;
    fn settings_general_min_panel_width(&self) -> &str;
    fn settings_general_session_retention(&self) -> &str;
    fn settings_general_bell(&self) -> &str;
    fn settings_general_resource_interval(&self) -> &str;

    // Settings modal — Editor fields
    fn settings_editor_tab_size(&self) -> &str;
    fn settings_editor_word_wrap(&self) -> &str;
    fn settings_editor_auto_indent(&self) -> &str;
    fn settings_editor_auto_close_brackets(&self) -> &str;
    fn settings_editor_show_git_diff(&self) -> &str;
    fn settings_editor_show_blame(&self) -> &str;
    fn settings_editor_large_file_threshold(&self) -> &str;

    // Settings modal — File Manager fields
    fn settings_fm_extended_view_width(&self) -> &str;
    fn settings_fm_content_search_max_size(&self) -> &str;
    fn settings_fm_dir_size_in_wide_view(&self) -> &str;
    fn settings_fm_dir_size_budget_ms(&self) -> &str;

    // Settings modal — Terminal fields
    fn settings_terminal_default_shell(&self) -> &str;

    // Settings modal — LSP fields
    fn settings_lsp_enabled(&self) -> &str;
    fn settings_lsp_auto_completion(&self) -> &str;
    fn settings_lsp_completion_delay(&self) -> &str;
    fn settings_lsp_hover_delay(&self) -> &str;
    fn settings_lsp_add_server(&self) -> &str;

    // Settings modal — Logging fields
    fn settings_logging_file_path(&self) -> &str;
    fn settings_logging_min_level(&self) -> &str;

    // Settings modal — VFS fields
    fn settings_vfs_connection_timeout(&self) -> &str;

    // Settings modal — Keybindings hints
    fn settings_kb_hint_bindings(&self) -> &str;
    fn settings_kb_hint_capturing(&self) -> &str;
    fn settings_kb_press_key(&self) -> &str;

    // Sessions
    fn sessions_title(&self) -> &str;
    fn sessions_current(&self) -> &str;
    fn sessions_new(&self) -> &str;
    fn sessions_switch(&self) -> &str;
    fn sessions_change_root(&self) -> &str;
    fn session_created(&self) -> &str;
    fn session_moved(&self) -> &str;

    // Directory picker
    fn directory_picker_create(&self) -> &str;
    fn directory_picker_move(&self) -> &str;
    fn directory_picker_cancel(&self) -> &str;

    // Directory switcher
    fn directory_switcher_title(&self) -> &str;
    fn directory_switcher_no_paths(&self) -> &str;
    fn directory_switcher_unsupported(&self) -> &str;
    fn directory_switcher_process_running(&self) -> &str;

    // Relative time
    fn time_just_now(&self) -> &str;
    /// Compact unit suffix for hours in a duration, e.g. `h`.
    fn time_short_hours(&self) -> &str;
    /// Compact unit suffix for minutes in a duration, e.g. `m`.
    fn time_short_minutes(&self) -> &str;
    /// Compact unit suffix for seconds in a duration, e.g. `s`.
    fn time_short_seconds(&self) -> &str;
    fn time_minutes_ago(&self, count: usize) -> String;
    fn time_hours_ago(&self, count: usize) -> String;
    fn time_days_ago(&self, count: usize) -> String;
    fn time_weeks_ago(&self, count: usize) -> String;
    fn time_months_ago(&self, count: usize) -> String;
    fn time_years_ago(&self, count: usize) -> String;

    // Status bar
    fn status_dir(&self) -> &str;
    fn status_file(&self) -> &str;
    fn status_mod(&self) -> &str;
    fn status_owner(&self) -> &str;
    /// Status-bar label for the entry's size, e.g. "Size:".
    fn status_size(&self) -> &str;
    fn status_selected(&self) -> &str;
    fn status_pos(&self) -> &str;
    fn status_tab(&self) -> &str;
    fn status_tab_modal_title(&self) -> &str;
    fn status_plain_text(&self) -> &str;
    fn status_readonly(&self) -> &str;
    fn status_terminal(&self) -> &str;
    fn status_layout(&self) -> &str;
    // UI elements
    fn ui_yes(&self) -> &str;
    fn ui_no(&self) -> &str;
    fn ui_ok(&self) -> &str;
    fn ui_cancel(&self) -> &str;
    fn ui_continue(&self) -> &str;
    fn ui_close(&self) -> &str;
    fn ui_hint_separator(&self) -> &str;

    // Checkboxes
    fn checkbox_executable(&self) -> &str;
    fn checkbox_create_symlink(&self) -> &str;
    fn checkbox_relative_symlink(&self) -> &str;

    // File size units
    fn size_bytes(&self) -> &str;
    fn size_kilobytes(&self) -> &str;
    fn size_megabytes(&self) -> &str;
    fn size_gigabytes(&self) -> &str;
    // File info modal
    fn file_info_title_file(&self, name: &str) -> String;
    fn file_info_title_directory(&self, name: &str) -> String;
    fn file_info_title_symlink(&self, name: &str) -> String;
    fn file_info_path(&self) -> &str;
    fn file_info_target(&self) -> &str;
    fn file_info_size(&self) -> &str;
    fn file_info_owner(&self) -> &str;
    fn file_info_group(&self) -> &str;
    fn file_info_created(&self) -> &str;
    fn file_info_modified(&self) -> &str;
    fn file_info_calculating(&self) -> &str;
    /// Immediate (non-recursive) child count of a directory, e.g. "42 items".
    fn file_info_items(&self, count: usize) -> String;
    fn file_info_git(&self) -> &str;
    fn file_info_git_uncommitted(&self, count: usize) -> String;
    fn file_info_git_ahead(&self, count: usize) -> String;
    fn file_info_git_behind(&self, count: usize) -> String;
    fn file_info_git_ignored(&self) -> &str;
    fn file_info_follow_symlink(&self) -> &str;
    fn perm_permissions(&self) -> &str;
    fn perm_owner(&self) -> &str;
    fn perm_group(&self) -> &str;
    fn perm_others(&self) -> &str;

    // File types
    fn file_type_directory(&self) -> &str;
    fn file_type_file(&self) -> &str;
    // Progress modal
    fn progress_scanning(&self) -> &str;
    fn progress_delete_title(&self) -> &str;
    fn progress_copy_title(&self) -> &str;
    fn progress_move_title(&self) -> &str;
    fn progress_resume(&self) -> &str;
    fn progress_suspend(&self) -> &str;
    fn progress_pause(&self) -> &str;
    fn progress_abort(&self) -> &str;
    fn progress_counting_files(&self) -> &str;
    fn progress_files_count(&self, current: usize, total: usize) -> String;
    fn progress_files_size(&self, count: &str, size: &str) -> String;
    fn progress_data_count(&self, current: &str, total: &str) -> String;
    fn progress_speed_eta(&self, speed: &str, eta: &str) -> String;
    fn progress_speed(&self, speed: &str) -> String;

    // Conflict modal
    fn conflict_directory_title(&self) -> &str;
    fn conflict_file_title(&self) -> &str;
    fn conflict_overwrite(&self) -> &str;
    fn conflict_skip(&self) -> &str;
    fn conflict_rename(&self) -> &str;
    fn conflict_overwrite_all(&self) -> &str;
    fn conflict_skip_all(&self) -> &str;
    fn conflict_rename_all(&self) -> &str;
    fn conflict_already_exists(&self, item_type: &str, name: &str) -> String;

    // Operation cancellation
    // Status messages
    fn status_config_saved(&self) -> &str;
    fn status_delete_failed(&self, error: &str) -> String;

    // Operation type labels (for operation cards)
    fn op_type_copy_upload(&self) -> &str;
    fn op_type_copy_download(&self) -> &str;
    fn op_type_move_upload(&self) -> &str;
    fn op_type_move_download(&self) -> &str;
    fn op_type_rename(&self) -> &str;
    fn op_type_command(&self) -> &str;
    fn op_type_scanning(&self) -> &str;
    fn op_found_count(&self, count: usize) -> String;
    fn op_files_progress(&self, current: usize, total: usize) -> String;
    fn op_data_progress(&self, current: &str, total: &str) -> String;
    fn op_speed_rate(&self, speed: &str) -> String;
    fn op_elapsed(&self, time: &str) -> String;

    // Modal titles
    fn modal_confirm_title(&self) -> &str;
    fn modal_error_title(&self) -> &str;

    // Git panel strings
    fn git_no_repo(&self) -> &str;
    fn git_branch_detached(&self) -> &str;
    fn git_refreshed(&self) -> &str;
    fn git_status_loading(&self) -> &str;
    fn git_staged_header(&self) -> &str;
    fn git_unstaged_header(&self) -> &str;
    fn git_stage_all_btn(&self) -> &str;
    fn git_unstage_all_btn(&self) -> &str;
    fn git_revert_all_btn(&self) -> &str;
    fn git_log_btn(&self) -> &str;
    fn git_revert_all_confirm(&self) -> &str;
    fn git_checkout_not_impl(&self) -> &str;
    fn git_no_remote_url(&self) -> &str;
    fn git_diff_staged_marker(&self) -> &str;
    fn git_pushing(&self) -> &str;
    fn git_pulling(&self) -> &str;
    fn git_action_files_fmt(&self, action: &str, count: usize) -> String;
    fn git_action_error_fmt(&self, action: &str, error: &str) -> String;
    fn git_switched_to_fmt(&self, branch: &str) -> String;
    fn git_checkout_error_fmt(&self, error: &str) -> String;
    fn git_init_failed_fmt(&self, error: &str) -> String;
    fn git_log_title_fmt(&self, repo: &str, branch: &str) -> String;
    fn git_diff_title_commit_fmt(
        &self,
        repo: &str,
        branch: &str,
        hash: &str,
        files: &str,
    ) -> String;
    fn git_diff_title_fmt(&self, repo: &str, branch: &str, files: &str) -> String;

    // Git commit info modal
    fn git_commit_info_title(&self, hash: &str) -> String;
    fn git_commit_author(&self) -> &str;
    fn git_commit_date(&self) -> &str;
    fn git_commit_message(&self) -> &str;
    fn git_commit_files(&self) -> &str;
    fn git_commit_files_modified(&self) -> &str;
    fn git_commit_files_added(&self) -> &str;
    fn git_commit_files_deleted(&self) -> &str;
    fn git_commit_lines(&self) -> &str;

    // Outline panel strings
    fn outline_title(&self) -> &str;
    fn outline_no_symbols(&self) -> &str;

    // Diagnostics panel strings
    fn diagnostics_title(&self) -> &str;
    fn diagnostics_no_items(&self) -> &str;
    fn diagnostics_filter_all(&self) -> &str;
    fn diagnostics_filter_errors(&self) -> &str;
    fn diagnostics_filter_ew(&self) -> &str;
    fn diagnostics_title_fmt(&self, errors: usize, warnings: usize) -> String;
    fn diagnostics_filter_fmt(&self, filter: &str, count: usize) -> String;

    // Terminal
    fn terminal_kill_confirm(&self) -> &str;

    // Operations panel
    fn operation_cancel_confirm(&self) -> &str;

    // Content replace
    fn replace_done_title(&self) -> &str;
    fn replace_done_fmt(&self, count: usize, files: usize) -> String;
    fn replace_confirm_fmt(&self, count: usize, files: usize) -> String;
    fn replace_no_files_selected(&self) -> &str;
    fn replace_selection_fmt(&self, selected: usize, total: usize, matches: usize) -> String;

    // Image panel
    fn panel_image(&self) -> &str;
    fn image_error_fmt(&self, error: &str) -> String;

    // Resource modals
    fn resource_cpu_top_title(&self) -> &str;
    fn resource_ram_top_title(&self) -> &str;
    fn resource_disk_title(&self) -> &str;
    fn resource_disk_free(&self) -> &str;
    fn resource_disk_used(&self) -> &str;
    fn resource_disk_total(&self) -> &str;
    fn resource_count(&self) -> &str;
    fn resource_net_title(&self) -> &str;

    // Calendar
    fn calendar_mon(&self) -> &str;
    fn calendar_tue(&self) -> &str;
    fn calendar_wed(&self) -> &str;
    fn calendar_thu(&self) -> &str;
    fn calendar_fri(&self) -> &str;
    fn calendar_sat(&self) -> &str;
    fn calendar_sun(&self) -> &str;
    fn calendar_january(&self) -> &str;
    fn calendar_february(&self) -> &str;
    fn calendar_march(&self) -> &str;
    fn calendar_april(&self) -> &str;
    fn calendar_may(&self) -> &str;
    fn calendar_june(&self) -> &str;
    fn calendar_july(&self) -> &str;
    fn calendar_august(&self) -> &str;
    fn calendar_september(&self) -> &str;
    fn calendar_october(&self) -> &str;
    fn calendar_november(&self) -> &str;
    fn calendar_december(&self) -> &str;

    // Database viewer panel strings
    fn db_connecting(&self) -> &str;
    fn db_loading(&self) -> &str;
    fn db_no_tables(&self) -> &str;
    fn db_no_table(&self) -> &str;
    fn db_no_database(&self) -> &str;
    fn db_select_table(&self) -> &str;
    fn db_select_database(&self) -> &str;
    fn db_rows_empty(&self) -> &str;
    fn db_total_unknown(&self) -> &str;
    fn db_copied(&self) -> &str;
    fn db_copied_cell(&self) -> &str;
    fn db_copied_row(&self) -> &str;
    fn db_copy_tsv(&self) -> &str;
    fn db_copy_json(&self) -> &str;
    fn db_copy_insert(&self) -> &str;
    fn db_filter_operator(&self) -> &str;
    fn db_filter_value(&self) -> &str;
    fn db_filter_hint(&self) -> &str;
    fn db_filter_title(&self) -> &str;
    fn db_filter_apply(&self) -> &str;
    fn db_filter_clear(&self) -> &str;
    fn db_filter_cancel(&self) -> &str;
    fn db_edit_needs_primary_key(&self) -> &str;
    fn db_edit_title(&self) -> &str;
    fn db_edit_save(&self) -> &str;
    fn db_edit_null_checkbox(&self) -> &str;
    fn db_edit_null_value(&self) -> &str;
    fn db_edit_saved(&self) -> &str;
    fn db_edit_row_gone(&self) -> &str;
    fn db_edit_failed_fmt(&self, error: &str) -> String;
    fn db_status_connecting_fmt(&self, label: &str) -> String;
    fn db_status_failed_fmt(&self, label: &str, error: &str) -> String;
    fn db_rows_range_fmt(&self, start: u64, end: u64) -> String;
    fn db_total_fmt(&self, total: i64) -> String;
    fn db_sort_fmt(&self, column: &str, arrow: &str) -> String;
    fn db_filter_count_fmt(&self, count: usize) -> String;
    fn db_connection_failed_fmt(&self, error: &str) -> String;
    fn db_auth_failed_fmt(&self, error: &str) -> String;
    fn db_filter_title_fmt(&self, column: &str) -> String;
    fn db_row_title_fmt(&self, table: &str) -> String;

    // VFS remote connections
}

/// Initialize translation system.
///
/// Returns Ok(()) on success, Err if translation loading fails completely
/// (including fallback to English).
pub fn init() -> anyhow::Result<()> {
    init_with_language("auto")
}

/// Initialize translation system with specified language.
///
/// Returns Ok(()) on success, Err if translation loading fails completely
/// (including fallback to English).
pub fn init_with_language(lang: &str) -> anyhow::Result<()> {
    let detected = if lang == "auto" || lang.is_empty() {
        detect_language()
    } else {
        lang.to_string()
    };

    let translation = runtime::RuntimeTranslation::new(&detected).or_else(|e| {
        log::warn!(
            "Failed to load translations for '{}': {}. Falling back to English",
            detected,
            e
        );
        runtime::RuntimeTranslation::new("en")
    })?;

    let leaked: &'static dyn Translation = Box::leak(Box::new(translation));
    if let Ok(mut guard) = TRANSLATION.write() {
        *guard = Some(leaked);
    }
    if let Ok(mut guard) = CURRENT_LANGUAGE.write() {
        *guard = detected;
    }
    LANGUAGE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// Set language at runtime (for live preview and language switching).
///
/// Returns Ok(()) on success, Err if language loading fails.
pub fn set_language(lang: &str) -> anyhow::Result<()> {
    let translation = runtime::RuntimeTranslation::new(lang)?;
    let leaked: &'static dyn Translation = Box::leak(Box::new(translation));

    if let Ok(mut guard) = TRANSLATION.write() {
        *guard = Some(leaked);
    }
    if let Ok(mut guard) = CURRENT_LANGUAGE.write() {
        *guard = lang.to_string();
    }
    LANGUAGE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// Monotonic counter incremented on every language change. Cheap to read (one
/// atomic load); caches of translated strings can store the value they were
/// built at and rebuild when it differs.
pub fn language_generation() -> u64 {
    LANGUAGE_GENERATION.load(std::sync::atomic::Ordering::Acquire)
}

/// Localized "N units ago" for an elapsed duration in `seconds`, using the
/// current translation. Buckets scale just-now → minutes → hours → days →
/// weeks → months → years. Shared by session listings and git blame so both
/// stay translated and consistent.
#[must_use]
pub fn relative_age(seconds: u64) -> String {
    let tr = t();
    match seconds {
        0..=59 => tr.time_just_now().to_string(),
        60..=3_599 => tr.time_minutes_ago((seconds / 60) as usize),
        3_600..=86_399 => tr.time_hours_ago((seconds / 3_600) as usize),
        86_400..=604_799 => tr.time_days_ago((seconds / 86_400) as usize),
        604_800..=2_591_999 => tr.time_weeks_ago((seconds / 604_800) as usize),
        2_592_000..=31_535_999 => tr.time_months_ago((seconds / 2_592_000) as usize),
        _ => tr.time_years_ago((seconds / 31_536_000) as usize),
    }
}

/// Localized compact duration for an elapsed-time counter: `5s`, `2m 5s`,
/// `1h 02m 05s`, with unit suffixes from the current translation.
///
/// Distinct from [`relative_age`], which says how long *ago* something
/// happened. This one counts how long something has been running, so the
/// units are suffixes rather than a phrase.
#[must_use]
pub fn compact_duration(seconds: u64) -> String {
    let tr = t();
    let (hours, minutes, secs) = (seconds / 3_600, (seconds % 3_600) / 60, seconds % 60);
    if hours > 0 {
        format!(
            "{hours}{} {minutes}{} {secs}{}",
            tr.time_short_hours(),
            tr.time_short_minutes(),
            tr.time_short_seconds()
        )
    } else if minutes > 0 {
        format!(
            "{minutes}{} {secs}{}",
            tr.time_short_minutes(),
            tr.time_short_seconds()
        )
    } else {
        format!("{secs}{}", tr.time_short_seconds())
    }
}

/// Get the current translation.
///
/// The application calls [`init`] / [`init_with_language`] at startup to select
/// the configured language. If `t()` is reached before that (e.g. in unit
/// tests, or an early code path), it lazily falls back to English rather than
/// panicking, so callers never crash on an uninitialized translation system.
pub fn t() -> &'static dyn Translation {
    if let Ok(guard) = TRANSLATION.read() {
        if let Some(t) = *guard {
            return t;
        }
    }
    // Not initialized yet — install the built-in English translation.
    let en: &'static dyn Translation = Box::leak(Box::new(
        runtime::RuntimeTranslation::new("en").expect("built-in English translations must load"),
    ));
    if let Ok(mut guard) = TRANSLATION.write() {
        if let Some(t) = *guard {
            return t; // another thread initialized it in the meantime
        }
        *guard = Some(en);
    }
    en
}

/// Get the current language code.
pub fn current_language() -> String {
    CURRENT_LANGUAGE
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| "en".to_string())
}

/// Get list of all supported languages with their native names.
/// Returns Vec of (code, native_name) tuples.
pub fn get_language_list() -> Vec<(&'static str, &'static str)> {
    SUPPORTED_LANGUAGES.to_vec()
}

/// Get the native name of a language by its code.
pub fn get_language_name(code: &str) -> Option<&'static str> {
    SUPPORTED_LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::{compact_duration, relative_age};

    // `relative_age` falls back to the built-in English translation when no
    // language is initialized, so these assert the English bucket wording.
    #[test]
    fn relative_age_buckets() {
        assert_eq!(relative_age(30), "just now");
        assert_eq!(relative_age(300), "5 minutes ago"); // 5 min
        assert_eq!(relative_age(7_200), "2 hours ago");
        assert_eq!(relative_age(2 * 86_400), "2 days ago");
        assert_eq!(relative_age(2 * 604_800), "2 weeks ago");
        assert_eq!(relative_age(3 * 2_592_000), "3 months ago");
        // Years bucket — the case git blame previously handled but session did not.
        assert_eq!(relative_age(3 * 31_536_000), "3 years ago");
        assert_eq!(relative_age(31_536_000), "1 year ago");
    }

    /// Elapsed counters used to hard-code `h`/`m`/`s`, which stayed English in
    /// every locale. The units now come from the translation; with none
    /// initialized these assert the English fallback.
    #[test]
    fn compact_duration_scales_units() {
        assert_eq!(compact_duration(0), "0s");
        assert_eq!(compact_duration(45), "45s");
        assert_eq!(compact_duration(60), "1m 0s");
        assert_eq!(compact_duration(125), "2m 5s");
        assert_eq!(compact_duration(3_600), "1h 0m 0s");
        assert_eq!(compact_duration(4_205), "1h 10m 5s");
    }
}
