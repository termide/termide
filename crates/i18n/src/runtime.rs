use super::{loader, Translation};
use std::collections::HashMap;

/// Runtime translation implementation that loads from TOML files.
///
/// For non-English languages, also loads the English dictionary as a fallback
/// so that missing keys degrade to English rather than rendering as empty.
pub struct RuntimeTranslation {
    strings: HashMap<String, String>,
    formats: HashMap<String, String>,
    plurals: HashMap<String, loader::PluralRules>,
    fallback_strings: HashMap<String, String>,
    fallback_formats: HashMap<String, String>,
}

impl RuntimeTranslation {
    pub fn new(lang: &str) -> anyhow::Result<Self> {
        let data = loader::load_language(lang)?;
        let (fallback_strings, fallback_formats) = if lang == "en" {
            (HashMap::new(), HashMap::new())
        } else {
            let en = loader::load_language("en")?;
            (en.strings, en.formats)
        };
        Ok(Self {
            strings: data.strings,
            formats: data.formats,
            plurals: data.plurals,
            fallback_strings,
            fallback_formats,
        })
    }

    fn get_string(&self, key: &str) -> &str {
        if let Some(s) = self.strings.get(key) {
            return s.as_str();
        }
        if let Some(s) = self.fallback_strings.get(key) {
            log::warn!("Missing translation key: {} (using English fallback)", key);
            return s.as_str();
        }
        log::warn!("Missing translation key: {}", key);
        ""
    }

    fn format(&self, key: &str, args: &[(&str, &str)]) -> String {
        let template = self
            .formats
            .get(key)
            .map(|s| s.as_str())
            .or_else(|| {
                let v = self.fallback_formats.get(key).map(|s| s.as_str());
                if v.is_some() {
                    log::warn!("Missing format key: {} (using English fallback)", key);
                }
                v
            })
            .unwrap_or_else(|| {
                log::warn!("Missing format key: {}", key);
                ""
            });
        let mut result = template.to_string();
        for (placeholder, value) in args {
            let pattern = format!("{{{}}}", placeholder);
            result = result.replace(&pattern, value);
        }
        result
    }

    fn pluralize(&self, count: usize, key: &str) -> &str {
        if let Some(rules) = self.plurals.get(key) {
            match count {
                1 => &rules.one,
                2..=4 if rules.few.is_some() => rules.few.as_deref().unwrap_or(&rules.other),
                _ => &rules.other,
            }
        } else if count == 1 {
            ""
        } else {
            "s"
        }
    }
}

/// Generates trivial translation methods of the shape
///   fn $name(&self) -> &str { self.get_string(stringify!($name)) }
/// for every comma-separated identifier.
macro_rules! i18n_get_string_methods {
    ($($name:ident),* $(,)?) => {
        $(
            fn $name(&self) -> &str {
                self.get_string(stringify!($name))
            }
        )*
    };
}

impl Translation for RuntimeTranslation {
    // Generate 393 trivial `fn name(&self) -> &str` wrappers over get_string("name").
    i18n_get_string_methods! {
        git_operation_cancelled,
        modal_yes,
        modal_ok,
        panel_help,
        panel_journal,
        panel_operations,
        no_active_operations,
        editor_close_unsaved,
        editor_close_unsaved_question,
        editor_save_and_close,
        editor_close_without_saving,
        editor_cancel,
        editor_close_external,
        editor_close_external_question,
        editor_overwrite_disk,
        editor_keep_disk_close,
        editor_reload_into_editor,
        editor_close_conflict,
        editor_close_conflict_question,
        editor_reload_from_disk,
        editor_search_no_matches,
        fm_goto_title,
        fm_goto_prompt,
        connection_cancelled_title,
        connection_error_title,
        db_reconnect,
        db_close_panel,
        connection_timeout_title,
        connection_timeout_message,
        vfs_reconnect,
        vfs_open_local,
        vfs_close_panel,
        app_quit_confirm,
        app_quit_title,
        help_global_keys,
        help_file_manager_keys,
        help_editor_keys,
        help_terminal_keys,
        help_desc_menu,
        help_desc_quit,
        help_desc_help,
        help_desc_close_panel,
        help_desc_escape_close,
        help_desc_select,
        help_desc_new_terminal,
        help_desc_home,
        help_desc_end,
        help_desc_page_scroll,
        help_desc_create_file,
        help_desc_create_dir,
        help_desc_copy,
        help_desc_move,
        help_desc_rename,
        help_section_panels,
        help_section_git_status,
        help_section_navigation,
        help_section_git_diff,
        help_section_git_log,
        help_desc_new_file_manager,
        help_desc_new_editor,
        help_desc_new_journal,
        help_desc_open_preferences,
        help_desc_open_sessions,
        help_desc_open_git_status,
        help_desc_open_outline,
        help_desc_open_diagnostics,
        help_desc_open_git_log,
        help_desc_toggle_stack,
        help_desc_swap_left,
        help_desc_swap_right,
        help_desc_panel_action_menu,
        panel_action_close,
        panel_action_split,
        panel_action_merge,
        panel_action_move_left,
        panel_action_move_right,
        panel_action_move_up,
        panel_action_move_down,
        help_desc_move_first,
        help_desc_move_last,
        help_desc_resize_smaller,
        help_desc_resize_larger,
        help_desc_toggle_fullscreen_panel,
        help_desc_panel_grow_vertical,
        help_desc_panel_shrink_vertical,
        help_desc_prev_group,
        help_desc_next_group,
        help_desc_prev_panel,
        help_desc_next_panel,
        help_desc_goto_panel,
        help_desc_save_as,
        help_desc_reload,
        help_desc_duplicate_line,
        help_desc_delete_line,
        help_desc_toggle_comment,
        help_desc_search_next,
        help_desc_search_prev,
        help_desc_replace,
        help_desc_replace_current,
        help_desc_replace_all,
        help_desc_trigger_completion,
        help_desc_show_hover,
        help_desc_goto_definition,
        help_desc_find_references,
        help_desc_rename_symbol,
        help_desc_code_action,
        lsp_rename_no_identifier,
        lsp_rename_unsaved_file,
        lsp_rename_no_changes,
        help_desc_delete_generic,
        help_desc_open_bookmark_add,
        help_desc_command_palette,
        help_desc_word_nav,
        help_desc_paragraph_nav,
        help_desc_view_file,
        help_desc_edit_file,
        help_desc_toggle_hidden,
        help_desc_open_external,
        help_desc_stage_file,
        help_desc_unstage_file,
        help_desc_terminal_copy,
        help_desc_terminal_paste,
        help_desc_scroll_up,
        help_desc_scroll_down,
        help_desc_scroll_top,
        help_desc_scroll_bottom,
        help_desc_move_up,
        help_desc_move_down,
        help_desc_scroll_half_up,
        help_desc_toggle_collapse,
        help_desc_open_file_editor,
        help_desc_view_commit_diff,
        help_desc_stage_unstage,
        help_desc_tree_search,
        help_desc_expand_dir,
        help_desc_collapse_dir,
        help_desc_word_select,
        help_desc_paragraph_select,
        help_desc_switch_focus,
        help_desc_open_in_browser,
        help_section_diagnostics,
        help_section_operations,
        help_section_outline,
        help_section_references,
        help_section_image,
        help_section_database,
        help_desc_db_sort,
        help_desc_db_filter,
        help_desc_db_clear_filter,
        help_desc_db_detail,
        help_desc_db_copy_cell,
        help_desc_db_copy_row,
        help_desc_toggle_filter,
        help_desc_pause_resume,
        help_desc_cancel_operation,
        help_desc_navigate,
        help_desc_copy_name,
        help_desc_close_image,
        help_desc_vim_panel_nav,
        help_section_viewers,
        help_desc_viewer_toggle,
        help_desc_viewer_goto,
        help_desc_viewer_search,
        help_desc_viewer_reload,
        help_desc_viewer_copy,
        help_desc_viewer_follow,
        help_desc_viewer_external,
        help_desc_viewer_history,
        status_file_reloaded,
        modal_create_file_title,
        modal_create_dir_title,
        modal_save_as_title,
        batch_result_file_copied,
        batch_result_file_moved,
        batch_result_error_copy,
        batch_result_error_move,
        batch_result_copied,
        batch_result_moved,
        menu_sessions,
        menu_windows,
        menu_commands,
        menu_commands_add,
        menu_copy_diagram,
        menu_save_diagram_as,
        menu_view_as_diagram,
        status_no_diagram_symbols,
        command_params_title,
        command_params_run,
        command_params_cancel,
        command_run_label,
        command_config_label_name,
        command_config_label_command,
        command_config_label_group,
        command_config_label_display_name,
        command_config_label_mode,
        command_config_label_hotkey,
        command_config_label_project,
        command_config_project_checkbox,
        command_config_hotkey_hint,
        command_config_hotkey_invalid,
        command_config_hotkey_conflict,
        command_config_button_create,
        command_config_button_save,
        command_config_button_edit_file,
        command_config_button_cancel,
        command_config_mode_terminal,
        command_config_mode_background,
        command_config_mode_report,
        command_config_group_root,
        menu_options,
        menu_quit,
        menu_bookmarks,
        bookmarks_add_bookmark,
        bookmarks_no_bookmarks,
        bookmarks_add_title,
        bookmarks_add_path,
        bookmarks_add_description,
        bookmarks_add_group,
        bookmarks_add_project,
        tools_files,
        tools_terminal,
        tools_editor,
        tools_git_status,
        tools_git_log,
        stash_new,
        stash_include_untracked,
        stash_created,
        stash_changes,
        stash_files,
        stash_more,
        stash_pop,
        stash_apply,
        stash_drop,
        stash_diff,
        git_stash_button,
        tools_journal,
        tools_diagnostics,
        tools_operations,
        tools_outline,
        tools_web,
        tools_web_prompt,
        options_help,
        git_action_diff,
        git_action_revert,
        git_action_close,
        git_action_init,
        git_action_commit,
        git_action_push,
        git_action_pull,
        git_revert_confirm,
        git_file_properties_title,
        git_props_path,
        git_props_status,
        git_props_size,
        git_props_diff,
        git_props_deleted,
        git_action_edit,
        git_operation_timed_out,
        preferences_themes,
        preferences_language,
        preferences_edit,
        settings_tab_keybindings,
        settings_btn_cancel,
        settings_general_resource_interval,
        settings_editor_large_file_threshold,
        settings_fm_content_search_max_size,
        settings_fm_dir_size_in_wide_view,
        settings_fm_dir_size_budget_ms,
        settings_terminal_default_shell,
        settings_lsp_add_server,
        settings_logging_min_level,
        settings_vfs_connection_timeout,
        sessions_current,
        sessions_new,
        sessions_switch,
        sessions_change_root,
        session_created,
        session_moved,
        directory_picker_create,
        directory_picker_move,
        directory_picker_cancel,
        directory_switcher_title,
        directory_switcher_no_paths,
        directory_switcher_unsupported,
        directory_switcher_process_running,
        settings_kb_hint_bindings,
        settings_kb_hint_capturing,
        settings_kb_press_key,
        sessions_title,
        time_just_now,
        status_dir,
        status_file,
        status_mod,
        status_owner,
        status_selected,
        status_pos,
        status_tab,
        status_tab_modal_title,
        status_plain_text,
        status_readonly,
        status_terminal,
        status_layout,
        ui_yes,
        ui_no,
        ui_ok,
        ui_cancel,
        ui_continue,
        ui_close,
        ui_hint_separator,
        checkbox_executable,
        checkbox_create_symlink,
        checkbox_relative_symlink,
        size_bytes,
        size_kilobytes,
        size_megabytes,
        size_gigabytes,
        file_info_path,
        file_info_target,
        file_info_size,
        file_info_owner,
        file_info_group,
        file_info_created,
        file_info_modified,
        file_info_calculating,
        file_info_git,
        file_info_git_ignored,
        file_info_follow_symlink,
        perm_permissions,
        perm_owner,
        perm_group,
        perm_others,
        file_type_directory,
        file_type_file,
        progress_scanning,
        progress_delete_title,
        progress_copy_title,
        progress_move_title,
        progress_resume,
        progress_suspend,
        progress_pause,
        progress_abort,
        progress_counting_files,
        conflict_directory_title,
        conflict_file_title,
        conflict_overwrite,
        conflict_skip,
        conflict_rename,
        conflict_overwrite_all,
        conflict_skip_all,
        conflict_rename_all,
        status_config_saved,
        op_type_copy_upload,
        op_type_copy_download,
        op_type_move_upload,
        op_type_move_download,
        op_type_rename,
        op_type_command,
        op_type_scanning,
        modal_confirm_title,
        modal_error_title,
        git_no_repo,
        git_branch_detached,
        git_refreshed,
        git_status_loading,
        git_staged_header,
        git_unstaged_header,
        git_stage_all_btn,
        git_unstage_all_btn,
        git_revert_all_btn,
        git_log_btn,
        git_revert_all_confirm,
        git_checkout_not_impl,
        git_no_remote_url,
        git_diff_staged_marker,
        git_pushing,
        git_pulling,
        git_commit_author,
        git_commit_date,
        git_commit_message,
        git_commit_files,
        git_commit_files_modified,
        git_commit_files_added,
        git_commit_files_deleted,
        git_commit_lines,
        outline_title,
        outline_no_symbols,
        diagnostics_title,
        diagnostics_no_items,
        diagnostics_filter_all,
        diagnostics_filter_errors,
        diagnostics_filter_ew,
        terminal_kill_confirm,
        operation_cancel_confirm,
        replace_done_title,
        replace_no_files_selected,
        panel_image,
        resource_cpu_top_title,
        resource_ram_top_title,
        resource_disk_title,
        resource_disk_free,
        resource_disk_used,
        resource_disk_total,
        resource_count,
        resource_net_title,        help_desc_new_session,
        help_desc_save,
        help_desc_undo,
        help_desc_redo,
        help_desc_search,
        help_desc_search_content,
        help_desc_select_all,
        help_desc_refresh,
        help_desc_go_parent,
        help_desc_go_home_dir,
        help_desc_switch_directory,
        help_desc_go_to_path,
        help_desc_edit_copy,
        help_desc_edit_cut,
        help_desc_edit_paste,
        help_desc_view_diff,
        help_desc_revert,
        help_desc_checkout,
        help_desc_copy_hash,
        help_desc_scroll_half_down,
        settings_tab_general,
        settings_tab_editor,
        settings_tab_file_manager,
        settings_tab_terminal,
        settings_tab_lsp,
        settings_tab_logging,
        settings_tab_vfs,
        settings_btn_apply,
        settings_btn_reset,
        settings_btn_create_project_override,
        settings_btn_remove_project_override,
        settings_remove_project_override_title,
        settings_remove_project_override_message,
        settings_general_vim_mode,
        settings_general_theme,
        settings_general_language,
        settings_general_icon_mode,
        settings_general_auto_stack_threshold,
        settings_general_min_panel_width,
        settings_general_session_retention,
        settings_general_bell,
        settings_editor_tab_size,
        settings_editor_word_wrap,
        settings_editor_auto_indent,
        settings_editor_auto_close_brackets,
        settings_editor_show_git_diff,
        settings_editor_show_blame,
        settings_fm_extended_view_width,
        settings_lsp_enabled,
        settings_lsp_auto_completion,
        settings_lsp_completion_delay,
        settings_lsp_hover_delay,
        settings_logging_file_path,
        calendar_mon,
        calendar_tue,
        calendar_wed,
        calendar_thu,
        calendar_fri,
        calendar_sat,
        calendar_sun,
        calendar_january,
        calendar_february,
        calendar_march,
        calendar_april,
        calendar_may,
        calendar_june,
        calendar_july,
        calendar_august,
        calendar_september,
        calendar_october,
        calendar_november,
        calendar_december,
        db_connecting,
        db_loading,
        db_no_tables,
        db_no_table,
        db_no_database,
        db_select_table,
        db_select_database,
        db_rows_empty,
        db_total_unknown,
        db_copied,
        db_copied_cell,
        db_copied_row,
        db_copy_tsv,
        db_copy_json,
        db_copy_insert,
        db_filter_operator,
        db_filter_value,
        db_filter_hint,
        db_filter_title,
        db_filter_apply,
        db_filter_clear,
        db_filter_cancel,
    }

    fn db_status_connecting_fmt(&self, label: &str) -> String {
        self.format("db_status_connecting_fmt", &[("label", label)])
    }

    fn db_status_failed_fmt(&self, label: &str, error: &str) -> String {
        self.format(
            "db_status_failed_fmt",
            &[("label", label), ("error", error)],
        )
    }

    fn db_rows_range_fmt(&self, start: u64, end: u64) -> String {
        self.format(
            "db_rows_range_fmt",
            &[("start", &start.to_string()), ("end", &end.to_string())],
        )
    }

    fn db_total_fmt(&self, total: i64) -> String {
        self.format("db_total_fmt", &[("total", &total.to_string())])
    }

    fn db_sort_fmt(&self, column: &str, arrow: &str) -> String {
        self.format("db_sort_fmt", &[("column", column), ("arrow", arrow)])
    }

    fn db_filter_count_fmt(&self, count: usize) -> String {
        self.format("db_filter_count_fmt", &[("count", &count.to_string())])
    }

    fn db_connection_failed_fmt(&self, error: &str) -> String {
        self.format("db_connection_failed_fmt", &[("error", error)])
    }

    fn db_auth_failed_fmt(&self, error: &str) -> String {
        self.format("db_auth_failed_fmt", &[("error", error)])
    }

    fn db_filter_title_fmt(&self, column: &str) -> String {
        self.format("db_filter_title_fmt", &[("column", column)])
    }

    fn db_row_title_fmt(&self, table: &str) -> String {
        self.format("db_row_title_fmt", &[("table", table)])
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        let plural = self.pluralize(count, "file");
        self.format(
            "fm_paste_confirm",
            &[
                ("count", &count.to_string()),
                ("mode", mode),
                ("dest", dest),
                ("plural", plural),
            ],
        )
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        self.format("fm_copy_prompt", &[("name", name)])
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        self.format("fm_move_prompt", &[("name", name)])
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        self.format("editor_file_opened", &[("filename", filename)])
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        self.format(
            "editor_search_match_info",
            &[
                ("current", &current.to_string()),
                ("total", &total.to_string()),
            ],
        )
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        let plural = self.pluralize(count, "file");
        self.format(
            "editor_deletion_marker",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    // LSP help descriptions
    fn lsp_rename_result(&self, count: usize) -> String {
        let plural = self.pluralize(count, "file");
        self.format(
            "lsp_rename_result_fmt",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    // Navigation help descriptions (static keys)
    // Git Diff help descriptions (static keys)
    // Git Log help descriptions (static keys)
    // Additional help descriptions (missing entries audit)
    fn status_file_created(&self, name: &str) -> String {
        self.format("status_file_created", &[("name", name)])
    }

    fn status_dir_created(&self, name: &str) -> String {
        self.format("status_dir_created", &[("name", name)])
    }

    fn status_file_saved(&self, name: &str) -> String {
        self.format("status_file_saved", &[("name", name)])
    }

    fn status_diagram_copied(&self, lines: usize) -> String {
        self.format("status_diagram_copied", &[("lines", &lines.to_string())])
    }

    fn status_error_save(&self, error: &str) -> String {
        self.format("status_error_save", &[("error", error)])
    }

    fn status_error_reload(&self, error: &str) -> String {
        self.format("status_error_reload", &[("error", error)])
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        self.format(
            "status_error_open_file",
            &[("name", name), ("error", error)],
        )
    }

    fn status_opening_external(&self, name: &str) -> String {
        self.format("status_opening_external", &[("name", name)])
    }

    fn modal_copy_single_title(&self, name: &str) -> String {
        self.format("modal_copy_single_title", &[("name", name)])
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        let element = self.pluralize(count, "element");
        self.format(
            "modal_copy_multiple_title",
            &[("count", &count.to_string()), ("element", element)],
        )
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        self.format("modal_move_single_title", &[("name", name)])
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        let element = self.pluralize(count, "element");
        self.format(
            "modal_move_multiple_title",
            &[("count", &count.to_string()), ("element", element)],
        )
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        self.format("modal_delete_single_title", &[("name", name)])
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        let element = self.pluralize(count, "element");
        self.format(
            "modal_delete_multiple_title",
            &[("count", &count.to_string()), ("element", element)],
        )
    }

    fn modal_copy_single_prompt(&self, name: &str) -> String {
        self.format("modal_copy_single_prompt", &[("name", name)])
    }

    fn modal_copy_multiple_prompt(&self, count: usize) -> String {
        let element = self.pluralize(count, "element");
        self.format(
            "modal_copy_multiple_prompt",
            &[("count", &count.to_string()), ("element", element)],
        )
    }

    fn modal_move_single_prompt(&self, name: &str) -> String {
        self.format("modal_move_single_prompt", &[("name", name)])
    }

    fn modal_move_multiple_prompt(&self, count: usize) -> String {
        let element = self.pluralize(count, "element");
        self.format(
            "modal_move_multiple_prompt",
            &[("count", &count.to_string()), ("element", element)],
        )
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        self.format("batch_result_skipped_fmt", &[("count", &count.to_string())])
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        self.format("batch_result_errors_fmt", &[("count", &count.to_string())])
    }

    fn git_init_success(&self, path: &str) -> String {
        self.get_string("git_init_success").replace("{path}", path)
    }

    fn git_commit_title(&self, count: usize, repo: &str, branch: &str) -> String {
        self.get_string("git_commit_title")
            .replace("{count}", &count.to_string())
            .replace("{repo}", repo)
            .replace("{branch}", branch)
    }

    fn git_status_added(&self) -> String {
        self.get_string("git_status_added").to_string()
    }

    fn git_status_deleted(&self) -> String {
        self.get_string("git_status_deleted").to_string()
    }

    fn git_status_modified(&self) -> String {
        self.get_string("git_status_modified").to_string()
    }

    fn git_status_renamed(&self) -> String {
        self.get_string("git_status_renamed").to_string()
    }

    fn git_status_untracked(&self) -> String {
        self.get_string("git_status_untracked").to_string()
    }

    fn git_push_in_progress(&self) -> String {
        self.get_string("git_push_in_progress").to_string()
    }

    fn git_pull_in_progress(&self) -> String {
        self.get_string("git_pull_in_progress").to_string()
    }

    fn git_fetch_in_progress(&self) -> String {
        self.get_string("git_fetch_in_progress").to_string()
    }

    fn git_push_success(&self) -> String {
        self.get_string("git_push_success").to_string()
    }

    fn git_push_failed(&self) -> String {
        self.get_string("git_push_failed").to_string()
    }

    fn git_pull_success(&self) -> String {
        self.get_string("git_pull_success").to_string()
    }

    fn git_pull_failed(&self) -> String {
        self.get_string("git_pull_failed").to_string()
    }

    fn git_completed(&self) -> String {
        self.get_string("git_completed").to_string()
    }

    fn theme_changed(&self, name: &str) -> String {
        self.format("theme_changed", &[("name", name)])
    }

    fn language_changed(&self, name: &str) -> String {
        self.format("language_changed", &[("name", name)])
    }

    // Settings modal — tabs
    // Settings modal — buttons
    // Settings modal — General fields
    // Settings modal — Editor fields
    // Settings modal — File Manager fields
    // Settings modal — Terminal fields
    // Settings modal — LSP fields
    // Settings modal — Logging fields
    // Settings modal — VFS fields
    // Settings modal — Keybindings hints
    fn time_minutes_ago(&self, count: usize) -> String {
        let plural = self.pluralize(count, "minute");
        self.format(
            "time_minutes_ago",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn time_hours_ago(&self, count: usize) -> String {
        let plural = self.pluralize(count, "hour");
        self.format(
            "time_hours_ago",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn time_days_ago(&self, count: usize) -> String {
        let plural = self.pluralize(count, "day");
        self.format(
            "time_days_ago",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn time_weeks_ago(&self, count: usize) -> String {
        let plural = self.pluralize(count, "week");
        self.format(
            "time_weeks_ago",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn time_months_ago(&self, count: usize) -> String {
        let plural = self.pluralize(count, "month");
        self.format(
            "time_months_ago",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn file_info_title_file(&self, name: &str) -> String {
        self.format("file_info_title_file", &[("name", name)])
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        self.format("file_info_title_directory", &[("name", name)])
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        self.format("file_info_title_symlink", &[("name", name)])
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        self.format(
            "file_info_git_uncommitted",
            &[("count", &count.to_string())],
        )
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        self.format("file_info_git_ahead", &[("count", &count.to_string())])
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        self.format("file_info_git_behind", &[("count", &count.to_string())])
    }

    fn progress_files_count(&self, current: usize, total: usize) -> String {
        self.format(
            "progress_files_count",
            &[
                ("current", &current.to_string()),
                ("total", &total.to_string()),
            ],
        )
    }

    fn progress_files_size(&self, count: &str, size: &str) -> String {
        self.format("progress_files_size", &[("count", count), ("size", size)])
    }

    fn progress_data_count(&self, current: &str, total: &str) -> String {
        self.format(
            "progress_data_count",
            &[("current", current), ("total", total)],
        )
    }

    fn progress_speed_eta(&self, speed: &str, eta: &str) -> String {
        self.format("progress_speed_eta", &[("speed", speed), ("eta", eta)])
    }

    fn progress_speed(&self, speed: &str) -> String {
        self.format("progress_speed", &[("speed", speed)])
    }

    fn conflict_already_exists(&self, item_type: &str, name: &str) -> String {
        self.format(
            "conflict_already_exists",
            &[("type", item_type), ("name", name)],
        )
    }

    fn status_delete_failed(&self, error: &str) -> String {
        self.format("status_delete_failed", &[("error", error)])
    }

    fn op_found_count(&self, count: usize) -> String {
        self.format("op_found_count", &[("count", &count.to_string())])
    }

    fn op_files_progress(&self, current: usize, total: usize) -> String {
        self.format(
            "op_files_progress",
            &[
                ("current", &current.to_string()),
                ("total", &total.to_string()),
            ],
        )
    }

    fn op_data_progress(&self, current: &str, total: &str) -> String {
        self.format(
            "op_data_progress",
            &[("current", current), ("total", total)],
        )
    }

    fn op_speed_rate(&self, speed: &str) -> String {
        self.format("op_speed_rate", &[("speed", speed)])
    }

    fn op_elapsed(&self, time: &str) -> String {
        self.format("op_elapsed", &[("time", time)])
    }

    fn git_action_files_fmt(&self, action: &str, count: usize) -> String {
        let plural = self.pluralize(count, "file");
        self.format(
            "git_action_files_fmt",
            &[
                ("action", action),
                ("count", &count.to_string()),
                ("plural", plural),
            ],
        )
    }

    fn git_action_error_fmt(&self, action: &str, error: &str) -> String {
        self.format(
            "git_action_error_fmt",
            &[("action", action), ("error", error)],
        )
    }

    fn git_switched_to_fmt(&self, branch: &str) -> String {
        self.format("git_switched_to_fmt", &[("branch", branch)])
    }

    fn git_checkout_error_fmt(&self, error: &str) -> String {
        self.format("git_checkout_error_fmt", &[("error", error)])
    }

    fn git_init_failed_fmt(&self, error: &str) -> String {
        self.format("git_init_failed_fmt", &[("error", error)])
    }

    fn git_log_title_fmt(&self, repo: &str, branch: &str) -> String {
        self.format("git_log_title_fmt", &[("repo", repo), ("branch", branch)])
    }

    fn git_diff_title_commit_fmt(
        &self,
        repo: &str,
        branch: &str,
        hash: &str,
        files: &str,
    ) -> String {
        self.format(
            "git_diff_title_commit_fmt",
            &[
                ("repo", repo),
                ("branch", branch),
                ("hash", hash),
                ("files", files),
            ],
        )
    }

    fn git_diff_title_fmt(&self, repo: &str, branch: &str, files: &str) -> String {
        self.format(
            "git_diff_title_fmt",
            &[("repo", repo), ("branch", branch), ("files", files)],
        )
    }

    fn git_commit_info_title(&self, hash: &str) -> String {
        self.format("git_commit_info_title", &[("hash", hash)])
    }

    fn diagnostics_title_fmt(&self, errors: usize, warnings: usize) -> String {
        self.format(
            "diagnostics_title_fmt",
            &[
                ("errors", &errors.to_string()),
                ("warnings", &warnings.to_string()),
            ],
        )
    }

    fn diagnostics_filter_fmt(&self, filter: &str, count: usize) -> String {
        self.format(
            "diagnostics_filter_fmt",
            &[("filter", filter), ("count", &count.to_string())],
        )
    }

    fn image_error_fmt(&self, error: &str) -> String {
        self.format("image_error_fmt", &[("error", error)])
    }

    fn replace_done_fmt(&self, count: usize, files: usize) -> String {
        self.format(
            "replace_done_fmt",
            &[("count", &count.to_string()), ("files", &files.to_string())],
        )
    }

    fn replace_confirm_fmt(&self, count: usize, files: usize) -> String {
        self.format(
            "replace_confirm_fmt",
            &[("count", &count.to_string()), ("files", &files.to_string())],
        )
    }

    fn replace_selection_fmt(&self, selected: usize, total: usize, matches: usize) -> String {
        self.format(
            "replace_selection_fmt",
            &[
                ("selected", &selected.to_string()),
                ("total", &total.to_string()),
                ("matches", &matches.to_string()),
            ],
        )
    }

    // Calendar
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Format keys must live in the TOML `[formats]` section (not `[strings]`),
    /// otherwise `format()` can't find them and returns an empty string. Guard
    /// the content-replace formats against that regression in every language.
    #[test]
    fn replace_format_keys_resolve_in_all_languages() {
        for lang in ["en", "ru", "zh"] {
            let t = RuntimeTranslation::new(lang).unwrap();

            let confirm = t.replace_confirm_fmt(2, 3);
            assert!(
                confirm.contains('2') && confirm.contains('3'),
                "{lang}: {confirm:?}"
            );

            let done = t.replace_done_fmt(2, 3);
            assert!(done.contains('2') && done.contains('3'), "{lang}: {done:?}");

            let sel = t.replace_selection_fmt(1, 4, 9);
            assert!(
                sel.contains('1') && sel.contains('4') && sel.contains('9'),
                "{lang}: {sel:?}"
            );
        }
    }
}
