use super::{loader, Translation};
use std::collections::HashMap;

/// Runtime translation implementation that loads from TOML files.
///
/// For non-English languages, also loads the English dictionary as a fallback
/// so that missing keys degrade to English rather than rendering as empty.
pub struct RuntimeTranslation {
    plural_category: fn(usize) -> PluralCategory,
    strings: HashMap<String, String>,
    formats: HashMap<String, String>,
    plurals: HashMap<String, loader::PluralRules>,
    fallback_strings: HashMap<String, String>,
    fallback_formats: HashMap<String, String>,
}

/// The form of a counted word, as CLDR names them; only the ones the
/// dictionaries spell out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluralCategory {
    One,
    Few,
    Other,
}

/// How `lang` picks the form of a counted word.
fn plural_category_for(lang: &str) -> fn(usize) -> PluralCategory {
    match lang {
        "ru" => east_slavic_plural,
        _ => one_other_plural,
    }
}

/// 1 → one, anything else → other: English and most languages here.
fn one_other_plural(count: usize) -> PluralCategory {
    if count == 1 {
        PluralCategory::One
    } else {
        PluralCategory::Other
    }
}

/// Russian: 1, 21, 31… → one; 2–4, 22–24… → few; 5–20, 25–30… → other.
fn east_slavic_plural(count: usize) -> PluralCategory {
    match (count % 10, count % 100) {
        (1, n) if n != 11 => PluralCategory::One,
        (2..=4, n) if !(12..=14).contains(&n) => PluralCategory::Few,
        _ => PluralCategory::Other,
    }
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
            plural_category: plural_category_for(lang),
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

    /// Both paste confirmations take the same slots and pluralize the same
    /// way; only the key differs, so they share one formatter.
    fn format_paste(&self, key: &str, count: usize, names: &str, dest: &str) -> String {
        let plural = self.pluralize(count, "file");
        self.format(
            key,
            &[
                ("count", &count.to_string()),
                ("dest", dest),
                ("names", names),
                ("plural", plural),
            ],
        )
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
    fn pluralize(&self, count: usize, key: &str) -> &str {
        if let Some(rules) = self.plurals.get(key) {
            match (self.plural_category)(count) {
                PluralCategory::One => &rules.one,
                PluralCategory::Few => rules.few.as_deref().unwrap_or(&rules.other),
                PluralCategory::Other => &rules.other,
            }
        } else if count == 1 {
            ""
        } else {
            "s"
        }
    }

    // Generate 395 trivial `fn name(&self) -> &str` wrappers over get_string("name").
    i18n_get_string_methods! {
        agent_input_placeholder,
        agent_input_placeholder_shell,
        agent_info_session,
        agent_info_log,
        agent_info_agent,
        agent_info_provider,
        agent_info_model,
        agent_info_mode,
        agent_info_directory,
        agent_info_created,
        agent_info_last_active,
        agent_info_compactions,
        agent_info_messages,
        agent_info_tokens,
        agent_info_context,
        agent_info_output_cleaned,
        agent_chip_agent,
        agent_chip_mode,
        agent_chip_reasoning,
        agent_chip_tools,
        agent_chip_connection,
        agent_chip_model,
        agent_chip_on,
        agent_chip_off,
        agent_banner_connection,
        agent_banner_model,
        agent_banner_tools,
        agent_banner_cwd,
        agent_cwd_title,
        agent_banner_sessions,
        agent_project_command,
        agent_hint_loop,
        agent_hint_goal,
        agent_notice_external_history,
        agent_model_request_dropped,
        settings_header_appearance,
        settings_header_input,
        settings_header_layout,
        settings_header_notifications,
        settings_header_performance,
        settings_header_instance,
        settings_header_typing,
        settings_header_display,
        settings_header_search,
        settings_header_general,
        settings_header_timing,
        settings_header_servers,
        settings_header_model,
        settings_header_permissions,
        settings_header_transcript,
        settings_header_web,
        settings_header_connections,
        settings_header_connection,
        settings_value_auto,
        settings_value_none,
        settings_value_unset,
        settings_value_no_limit,
        palette_category_panels,
        palette_category_git,
        palette_category_navigation,
        palette_category_panel_management,
        palette_category_application,
        palette_category_commands,
        command_report_no_output,
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
        help_desc_open_projects,
        help_desc_open_git_status,
        help_desc_open_outline,
        help_desc_open_agent,
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
        help_desc_cycle_project,
        help_desc_goto_project,
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
        help_desc_open_path,
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
        help_desc_image_zoom,
        help_desc_image_fit,
        help_desc_image_pan,
        help_desc_vim_panel_nav,
        help_section_viewers,
        help_desc_viewer_toggle,
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
        menu_projects,
        menu_windows,
        menu_commands,
        menu_commands_add,
        menu_copy_diagram,
        menu_save_diagram_as,
        menu_view_as_diagram,
        menu_save_page_as_markdown,
        viewer_loading,
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
        menu_ai,
        menu_ai_agents,
        menu_ai_sessions,
        menu_ai_skills,
        menu_ai_prompts,
        menu_ai_new_project,
        menu_ai_new_global,
        ai_create_agent_title,
        ai_create_skill_title,
        ai_create_prompt_title,
        ai_rename_title,
        ai_delete_title,
        ai_name_hint,
        ai_name_invalid,
        ai_name_exists,
        ai_empty,
        ai_delete_session_title,
        ai_session_untitled,
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
        tools_agent,
        panel_agent,
        agent_rename,
        agent_delete_session,
        agent_fork_session,
        agent_rename_prompt,
        agent_new_session,
        agent_resume,
        agent_no_sessions,
        agent_not_configured,
        agent_change_model,
        agent_change_mode,
        agent_change_reasoning,
        agent_model_prompt,
        agent_model_other,
        agent_models_loading,
        agent_mode_ask,
        agent_mode_plan,
        agent_mode_edit,
        agent_mode_configured,
        agent_mode_auto,
        agent_mode_all,
        agent_perm_note_rules_denied,
        agent_perm_note_plan,
        agent_perm_note_hook_allowed,
        agent_perm_note_hook_denied,
        agent_perm_note_unattended,
        agent_perm_note_user_once,
        agent_perm_note_user_ran,
        agent_perm_note_user_session,
        agent_perm_note_user_project,
        agent_perm_note_user_global,
        agent_perm_note_user_denied,
        agent_perm_note_user_denied_session,
        agent_show_prompt,
        agent_session_info,
        agent_perm_allow_once,
        agent_perm_allow_session,
        agent_perm_allow_always,
        agent_perm_allow_always_global,
        agent_perm_deny,
        agent_perm_deny_session,
        agent_perm_part_once,
        agent_perm_deny_reason,
        agent_perm_stop,
        agent_question_title,
        agent_question_own_answer,
        agent_question_decline,
        agent_question_submit,
        agent_undo_restore,
        agent_undo_keep,
        agent_rewind_conversation_only,
        agent_rewind_files_only,
        agent_plan_carry_title,
        agent_plan_clean_edits,
        agent_plan_accept_edits,
        agent_plan_configured,
        agent_plan_keep,
        agent_handoff_ready_title,
        agent_handoff_save,
        agent_handoff_new_session,
        agent_handoff_dismiss,
        agent_cmd_run_once,
        agent_cmd_run_session,
        agent_cmd_run_always,
        agent_cmd_dont_run,
        agent_cmd_desc_compact,
        agent_cmd_desc_undo,
        agent_cmd_desc_new,
        agent_cmd_desc_fork,
        agent_cmd_desc_clear,
        agent_cmd_desc_rename,
        agent_cmd_desc_pause,
        agent_cmd_desc_continue,
        agent_cmd_desc_loop,
        agent_cmd_desc_goal,
        agent_cmd_desc_handoff,
        agent_cmd_desc_usage,
        agent_cmd_desc_mcp,
        agent_save_chat,
        agent_export_you,
        agent_export_title,
        agent_notice_chat_empty,
        agent_toolset_buttons_hint,
        agent_hint_mcp,
        agent_notice_mcp_none,
        agent_notice_mcp_usage,
        agent_mcp_status_connecting,
        agent_mcp_status_needs_login,
        agent_mcp_status_signing_in,
        agent_cmd_desc_prompt,
        agent_notice_busy,
        agent_notice_no_log_to_name,
        agent_notice_will_pause,
        agent_notice_nothing_to_pause,
        agent_notice_already_running,
        agent_notice_nothing_to_continue,
        agent_notice_loop_stopped,
        agent_notice_loop_usage,
        agent_notice_goal_stopped,
        agent_notice_goal_usage,
        agent_state_queued,
        agent_notice_goal_checking,
        agent_notice_handoff_preparing,
        agent_notice_stopping,
        agent_notice_goal_stopped_failed,
        agent_notice_compacting,
        agent_notice_no_model_choices,
        agent_notice_plan_no_request,
        agent_notice_nothing_to_open,
        agent_notice_nothing_to_undo,
        agent_notice_fork_no_session,
        agent_notice_nothing_to_rollback,
        agent_notice_rewound,
        agent_notice_command_running,
        agent_notice_command_dropped,
        agent_notice_bang_unavailable,
        agent_notice_bang_running,
        agent_notice_bang_dropped,
        agent_notice_bang_failed,
        agent_notice_bang_done,
        agent_notice_bang_stopped,
        agent_suggest_title,
        agent_suggest_by_agent,
        agent_suggest_run,
        agent_suggest_edit,
        agent_suggest_copy,
        agent_suggest_dismiss,
        agent_suggest_denied_plan,
        agent_suggest_denied_rule,
        agent_notice_clipboard_failed,
        agent_notice_goal_reached,
        agent_notice_looping,
        agent_change_agent,
        agent_prompts,
        agent_no_prompts,
        agent_undo,
        agent_thinking,
        agent_unit_secs,
        agent_unit_mins,
        agent_unit_hours,
        agent_unit_days,
        agent_unit_tok_per_sec,
        agent_tool_bash,
        agent_pick_connection,
        agent_delete_this_session,
        agent_fork_this_session,
        agent_notice_connection_before_first,
        agent_notice_model_pending,
        agent_toolset_title,
        agent_toolset_prompt,
        agent_toolset_builtin,
        agent_toolset_skills,
        agent_toolset_note_refused,
        agent_toolset_note_new_session,
        agent_tool_read,
        agent_tool_write,
        agent_tool_edit,
        agent_tool_fetch,
        agent_tool_web_search,
        agent_tool_recall,
        agent_tool_skill,
        agent_tool_task,
        agent_tool_mcp,
        agent_tool_question,
        agent_slash_kind_builtin,
        agent_slash_kind_template,
        agent_slash_kind_script,
        agent_slash_kind_skill,
        settings_tab_agent,
        settings_agent_provider,
        settings_agent_base_url,
        settings_agent_model,
        settings_agent_api_key_env,
        settings_agent_context_window,
        settings_agent_max_tokens,
        settings_agent_reasoning,
        settings_agent_autofold,
        settings_agent_fold_immediately,
        settings_agent_fold_on_finish,
        settings_agent_fold_never,
        settings_agent_permission_mode,
        settings_agent_auto_reviewer,
        settings_agent_auto_reviewer_session,
        settings_ai_add_connection,
        settings_ai_delete_connection,
        settings_ai_connection_back,
        settings_ai_model_auto,
        settings_ai_connection_hint_claude_code,
        settings_ai_connection_hint_codex,
        settings_ai_connection_hint_gemini_cli,
        settings_ai_connection_name,
        settings_ai_connection_default,
        settings_ai_connection_prefill_progress,
        settings_ai_connection_reasoning_param,
        settings_ai_connection_name_taken,
        settings_web_backend,
        menu_ai_show_browser,
        menu_ai_hide_browser,
        settings_web_engine,
        settings_web_display,
        settings_web_chrome_path,
        tools_open,
        tools_open_prompt,
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
        settings_title,
        settings_kb_global,
        settings_kb_editor,
        settings_kb_file_manager,
        settings_kb_git_status,
        settings_kb_git_diff,
        settings_kb_git_log,
        settings_kb_terminal,
        settings_kb_database,
        settings_kb_viewer,
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
        projects_new,
        projects_switch,
        projects_change_root,
        projects_delete_title,
        project_created,
        project_moved,
        projects_close_title,
        projects_close_warning,
        projects_already_open,
        projects_already_current,
        detach_instance,
        detach_not_detached_instance,
        detach_failed,
        settings_general_always_detachable,
        help_desc_detach_instance,
        directory_picker_create,
        directory_picker_move,
        directory_picker_select,
        directory_picker_cancel,
        directory_switcher_title,
        directory_switcher_no_paths,
        directory_switcher_unsupported,
        directory_switcher_process_running,
        settings_kb_hint_bindings,
        settings_kb_hint_capturing,
        settings_kb_press_key,
        projects_title,
        time_just_now,
        time_short_hours,
        time_short_minutes,
        time_short_seconds,
        status_dir,
        status_file,
        status_mod,
        status_owner,
        status_size,
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
        fm_archive_read_only,
        fm_cut_local_only,
        help_desc_pack,
        op_type_pack,
        modal_pack_title,
        fm_pack_local_only,
        modal_archive_password_title,
        status_vfs_resolving_link,
        status_vfs_loading,
        status_vfs_connected,
        status_vfs_cancelled,
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
        git_log_loading,
        git_checkout_btn,
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
        resource_disk_type,
        resource_count,
        resource_net_title,        help_desc_new_project,
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
        settings_general_project_retention,
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
        db_edit_needs_primary_key,
        db_edit_title,
        db_edit_save,
        db_edit_null_checkbox,
        db_edit_null_value,
        db_edit_saved,
        db_edit_row_gone,
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

    fn agent_permission_run_fmt(&self, tool: &str) -> String {
        self.format("agent_permission_run_fmt", &[("tool", tool)])
    }

    fn agent_delete_confirm_fmt(&self, label: &str) -> String {
        self.format("agent_delete_confirm_fmt", &[("label", label)])
    }

    fn agent_fork_confirm_fmt(&self, label: &str) -> String {
        self.format("agent_fork_confirm_fmt", &[("label", label)])
    }

    fn agent_notice_cannot_fork_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_fork_fmt", &[("error", error)])
    }

    fn agent_undo_confirm_fmt(&self, changed: &str) -> String {
        self.format("agent_undo_confirm_fmt", &[("changed", changed)])
    }

    fn agent_undo_changed_files_fmt(&self, count: usize, files: &str) -> String {
        self.format(
            "agent_undo_changed_files_fmt",
            &[("count", &count.to_string()), ("files", files)],
        )
    }

    fn agent_command_run_title_fmt(&self, name: &str, path: &str) -> String {
        self.format(
            "agent_command_run_title_fmt",
            &[("name", name), ("path", path)],
        )
    }

    fn agent_notice_cannot_continue_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_continue_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_start_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_start_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_check_goal_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_check_goal_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_handoff_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_handoff_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_write_handoff_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_write_handoff_fmt", &[("error", error)])
    }

    fn agent_notice_compaction_failed_fmt(&self, error: &str) -> String {
        self.format("agent_notice_compaction_failed_fmt", &[("error", error)])
    }

    fn agent_notice_goal_check_failed_fmt(&self, error: &str) -> String {
        self.format("agent_notice_goal_check_failed_fmt", &[("error", error)])
    }

    fn agent_notice_handoff_failed_fmt(&self, error: &str) -> String {
        self.format("agent_notice_handoff_failed_fmt", &[("error", error)])
    }

    fn agent_notice_model_list_unavailable_fmt(&self, error: &str) -> String {
        self.format(
            "agent_notice_model_list_unavailable_fmt",
            &[("error", error)],
        )
    }

    fn agent_notice_mcp_error_fmt(&self, source: &str, error: &str) -> String {
        self.format(
            "agent_notice_mcp_error_fmt",
            &[("source", source), ("error", error)],
        )
    }

    fn agent_notice_connection_fmt(&self, name: &str) -> String {
        self.format("agent_notice_connection_fmt", &[("name", name)])
    }

    fn agent_notice_no_connection_fmt(&self, name: &str) -> String {
        self.format("agent_notice_no_connection_fmt", &[("name", name)])
    }

    fn agent_toolset_mcp_fmt(&self, server: &str) -> String {
        self.format("agent_toolset_mcp_fmt", &[("server", server)])
    }

    fn agent_notice_mcp_reconnected_fmt(&self, source: &str, count: usize) -> String {
        self.format(
            "agent_notice_mcp_reconnected_fmt",
            &[("source", source), ("count", &count.to_string())],
        )
    }

    fn agent_notice_mcp_tools_on_fmt(&self, source: &str, on: usize, count: usize) -> String {
        self.format(
            "agent_notice_mcp_tools_on_fmt",
            &[
                ("source", source),
                ("on", &on.to_string()),
                ("count", &count.to_string()),
            ],
        )
    }

    fn agent_notice_toolset_changed_fmt(&self, off: &str, on: &str) -> String {
        self.format(
            "agent_notice_toolset_changed_fmt",
            &[("off", off), ("on", on)],
        )
    }

    fn agent_notice_mcp_connected_fmt(&self, source: &str, count: usize) -> String {
        self.format(
            "agent_notice_mcp_connected_fmt",
            &[("source", source), ("count", &count.to_string())],
        )
    }

    fn agent_mcp_status_ready_fmt(&self, count: usize) -> String {
        self.format(
            "agent_mcp_status_ready_fmt",
            &[("count", &count.to_string())],
        )
    }

    fn agent_mcp_status_failed_fmt(&self, error: &str) -> String {
        self.format("agent_mcp_status_failed_fmt", &[("error", error)])
    }

    fn agent_notice_mcp_status_fmt(&self, source: &str, status: &str) -> String {
        self.format(
            "agent_notice_mcp_status_fmt",
            &[("source", source), ("status", status)],
        )
    }

    fn agent_notice_mcp_reload_fmt(&self, started: &str, removed: &str, kept: &str) -> String {
        self.format(
            "agent_notice_mcp_reload_fmt",
            &[("started", started), ("removed", removed), ("kept", kept)],
        )
    }

    fn agent_notice_mcp_needs_login_fmt(&self, source: &str) -> String {
        self.format("agent_notice_mcp_needs_login_fmt", &[("source", source)])
    }

    fn agent_notice_mcp_login_started_fmt(&self, source: &str, url: &str) -> String {
        self.format(
            "agent_notice_mcp_login_started_fmt",
            &[("source", source), ("url", url)],
        )
    }

    fn agent_notice_mcp_gone_fmt(&self, source: &str) -> String {
        self.format("agent_notice_mcp_gone_fmt", &[("source", source)])
    }

    fn agent_notice_mcp_updated_fmt(&self, source: &str, count: usize) -> String {
        self.format(
            "agent_notice_mcp_updated_fmt",
            &[("source", source), ("count", &count.to_string())],
        )
    }

    fn agent_notice_mcp_logout_fmt(&self, source: &str) -> String {
        self.format("agent_notice_mcp_logout_fmt", &[("source", source)])
    }

    fn agent_notice_mcp_no_login_fmt(&self, source: &str) -> String {
        self.format("agent_notice_mcp_no_login_fmt", &[("source", source)])
    }

    fn agent_notice_no_agent_fmt(&self, name: &str) -> String {
        self.format("agent_notice_no_agent_fmt", &[("name", name)])
    }

    fn agent_notice_agent_fmt(&self, name: &str) -> String {
        self.format("agent_notice_agent_fmt", &[("name", name)])
    }

    fn agent_notice_cannot_switch_agent_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_switch_agent_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_switch_model_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_switch_model_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_change_reasoning_fmt(&self, error: &str) -> String {
        self.format(
            "agent_notice_cannot_change_reasoning_fmt",
            &[("error", error)],
        )
    }

    fn agent_notice_reasoning_fmt(&self, level: &str) -> String {
        self.format("agent_notice_reasoning_fmt", &[("level", level)])
    }

    fn agent_notice_model_fmt(&self, id: &str) -> String {
        self.format("agent_notice_model_fmt", &[("id", id)])
    }

    fn agent_notice_cannot_open_session_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_open_session_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_open_block_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_open_block_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_undo_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_undo_fmt", &[("error", error)])
    }

    fn agent_notice_cannot_write_prompt_fmt(&self, error: &str) -> String {
        self.format("agent_notice_cannot_write_prompt_fmt", &[("error", error)])
    }

    fn agent_notice_retry_fmt(
        &self,
        attempt: usize,
        max: usize,
        delay_ms: u64,
        error: &str,
    ) -> String {
        self.format(
            "agent_notice_retry_fmt",
            &[
                ("attempt", &attempt.to_string()),
                ("max", &max.to_string()),
                ("delay_ms", &delay_ms.to_string()),
                ("error", error),
            ],
        )
    }

    fn agent_notice_compacted_fmt(&self, tokens: u64, kept: usize) -> String {
        self.format(
            "agent_notice_compacted_fmt",
            &[("tokens", &tokens.to_string()), ("kept", &kept.to_string())],
        )
    }

    fn agent_notice_handoff_written_fmt(&self, path: &str) -> String {
        self.format("agent_notice_handoff_written_fmt", &[("path", path)])
    }

    fn agent_notice_no_command_fmt(&self, name: &str, available: &str) -> String {
        self.format(
            "agent_notice_no_command_fmt",
            &[("name", name), ("available", available)],
        )
    }

    fn agent_notice_slash_shadowed_fmt(&self, name: &str, runs: &str, hidden: &str) -> String {
        self.format(
            "agent_notice_slash_shadowed_fmt",
            &[("name", name), ("runs", runs), ("hidden", hidden)],
        )
    }

    fn agent_notice_slash_skill_hint_fmt(&self, name: &str) -> String {
        self.format("agent_notice_slash_skill_hint_fmt", &[("name", name)])
    }

    fn agent_notice_unknown_key_fmt(&self, file: &str, key: &str) -> String {
        self.format(
            "agent_notice_unknown_key_fmt",
            &[("file", file), ("key", key)],
        )
    }

    fn agent_notice_unknown_tool_text_fmt(&self, file: &str) -> String {
        self.format("agent_notice_unknown_tool_text_fmt", &[("file", file)])
    }

    fn agent_notice_empty_tool_text_fmt(&self, file: &str) -> String {
        self.format("agent_notice_empty_tool_text_fmt", &[("file", file)])
    }

    fn agent_notice_loop_stopped_max_fmt(&self, count: usize) -> String {
        self.format(
            "agent_notice_loop_stopped_max_fmt",
            &[("count", &count.to_string())],
        )
    }

    fn agent_notice_goal_stopped_max_fmt(&self, count: usize) -> String {
        self.format(
            "agent_notice_goal_stopped_max_fmt",
            &[("count", &count.to_string())],
        )
    }

    fn agent_notice_goal_working_fmt(&self, goal: &str) -> String {
        self.format("agent_notice_goal_working_fmt", &[("goal", goal)])
    }

    fn agent_notice_looping_every_fmt(&self, interval: &str) -> String {
        self.format("agent_notice_looping_every_fmt", &[("interval", interval)])
    }

    fn agent_notice_goal_reached_reason_fmt(&self, reason: &str) -> String {
        self.format(
            "agent_notice_goal_reached_reason_fmt",
            &[("reason", reason)],
        )
    }

    fn agent_notice_command_denied_fmt(&self, name: &str) -> String {
        self.format("agent_notice_command_denied_fmt", &[("name", name)])
    }

    fn agent_notice_rolled_back_fmt(&self, count: usize, plural: &str) -> String {
        self.format(
            "agent_notice_rolled_back_fmt",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn agent_notice_files_restored_fmt(&self, count: usize, plural: &str) -> String {
        self.format(
            "agent_notice_files_restored_fmt",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn agent_rewind_confirm_fmt(&self, message: &str, changed: &str) -> String {
        self.format(
            "agent_rewind_confirm_fmt",
            &[("message", message), ("changed", changed)],
        )
    }

    fn agent_notice_undid_fmt(&self, count: usize, plural: &str) -> String {
        self.format(
            "agent_notice_undid_fmt",
            &[("count", &count.to_string()), ("plural", plural)],
        )
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

    fn db_edit_failed_fmt(&self, error: &str) -> String {
        self.format("db_edit_failed_fmt", &[("error", error)])
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

    fn fm_paste_confirm(&self, count: usize, names: &str, dest: &str) -> String {
        self.format_paste("fm_paste_confirm", count, names, dest)
    }

    fn fm_paste_move_confirm(&self, count: usize, names: &str, dest: &str) -> String {
        self.format_paste("fm_paste_move_confirm", count, names, dest)
    }

    fn fm_copy_into_itself(&self, name: &str) -> String {
        self.format("fm_copy_into_itself", &[("name", name)])
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
        let plural = self.pluralize(count, "line");
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

    fn fm_pack_prompt(&self, name: &str) -> String {
        self.format("fm_pack_prompt", &[("name", name)])
    }

    fn fm_pack_prompt_multiple(&self, count: usize) -> String {
        self.format("fm_pack_prompt_multiple", &[("count", &count.to_string())])
    }

    fn fm_pack_unknown_format(&self, name: &str) -> String {
        self.format("fm_pack_unknown_format", &[("name", name)])
    }

    fn fm_pack_exists(&self, name: &str) -> String {
        self.format("fm_pack_exists", &[("name", name)])
    }

    fn fm_archive_password_prompt(&self, name: &str) -> String {
        self.format("fm_archive_password_prompt", &[("name", name)])
    }

    fn fm_archive_password_wrong(&self, name: &str) -> String {
        self.format("fm_archive_password_wrong", &[("name", name)])
    }

    fn status_vfs_connecting(&self, host: &str) -> String {
        self.format("status_vfs_connecting", &[("host", host)])
    }

    fn status_vfs_opening(&self, name: &str) -> String {
        self.format("status_vfs_opening", &[("name", name)])
    }

    fn status_item_count(&self, count: usize) -> String {
        let element = self.pluralize(count, "element");
        self.format(
            "status_item_count",
            &[("count", &count.to_string()), ("element", element)],
        )
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
        self.format("git_init_success", &[("path", path)])
    }
    fn agent_thought_chars(&self, count: usize) -> String {
        self.format("agent_thought_chars", &[("count", &count.to_string())])
    }

    fn agent_more_lines_above(&self, count: usize) -> String {
        self.format("agent_more_lines_above", &[("count", &count.to_string())])
    }

    fn agent_more_lines(&self, count: usize) -> String {
        self.format("agent_more_lines", &[("count", &count.to_string())])
    }

    fn agent_state_queued_more(&self, count: usize) -> String {
        self.format("agent_state_queued_more", &[("count", &count.to_string())])
    }

    fn agent_paste_placeholder_fmt(&self, n: usize, what: &str) -> String {
        self.format(
            "agent_paste_placeholder_fmt",
            &[("n", &n.to_string()), ("what", what)],
        )
    }

    fn agent_paste_lines_fmt(&self, count: usize) -> String {
        self.format("agent_paste_lines_fmt", &[("count", &count.to_string())])
    }

    fn agent_paste_chars_fmt(&self, count: usize) -> String {
        self.format("agent_paste_chars_fmt", &[("count", &count.to_string())])
    }

    fn agent_project_command_fmt(&self, description: &str) -> String {
        self.format("agent_project_command_fmt", &[("description", description)])
    }

    fn agent_running_command_fmt(&self, name: &str) -> String {
        self.format("agent_running_command_fmt", &[("name", name)])
    }

    fn agent_notice_bang_running_cmd_fmt(&self, command: &str) -> String {
        self.format("agent_notice_bang_running_cmd_fmt", &[("command", command)])
    }

    fn agent_suggest_why_fmt(&self, why: &str) -> String {
        self.format("agent_suggest_why_fmt", &[("why", why)])
    }

    fn agent_suggest_cwd_fmt(&self, cwd: &str) -> String {
        self.format("agent_suggest_cwd_fmt", &[("cwd", cwd)])
    }

    fn agent_notice_external_failed_fmt(&self, error: &str) -> String {
        self.format("agent_notice_external_failed_fmt", &[("error", error)])
    }

    fn agent_perm_note_reviewer_allowed_fmt(&self, reason: &str) -> String {
        self.format(
            "agent_perm_note_reviewer_allowed_fmt",
            &[("reason", reason)],
        )
    }

    fn agent_perm_note_reviewer_blocked_fmt(&self, reason: &str) -> String {
        self.format(
            "agent_perm_note_reviewer_blocked_fmt",
            &[("reason", reason)],
        )
    }

    fn agent_perm_note_user_denied_reason_fmt(&self, reason: &str) -> String {
        self.format(
            "agent_perm_note_user_denied_reason_fmt",
            &[("reason", reason)],
        )
    }

    fn agent_queued_fmt(&self, count: usize) -> String {
        self.format("agent_queued_fmt", &[("count", &count.to_string())])
    }

    fn settings_value_default_fmt(&self, value: &str) -> String {
        self.format("settings_value_default_fmt", &[("value", value)])
    }

    fn projects_delete_failed_fmt(&self, error: &str) -> String {
        self.format("projects_delete_failed_fmt", &[("error", error)])
    }

    fn command_edit_title_fmt(&self, name: &str) -> String {
        self.format("command_edit_title_fmt", &[("name", name)])
    }

    fn command_run_failed_fmt(&self, error: &str) -> String {
        self.format("command_run_failed_fmt", &[("error", error)])
    }

    fn command_name_taken_fmt(&self, name: &str) -> String {
        self.format("command_name_taken_fmt", &[("name", name)])
    }

    fn git_commit_title(&self, count: usize, repo: &str, branch: &str) -> String {
        self.format(
            "git_commit_title",
            &[
                ("count", &count.to_string()),
                ("repo", repo),
                ("branch", branch),
            ],
        )
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

    fn time_years_ago(&self, count: usize) -> String {
        let plural = self.pluralize(count, "year");
        self.format(
            "time_years_ago",
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

    fn file_info_items(&self, count: usize) -> String {
        self.format("file_info_items", &[("count", &count.to_string())])
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

    fn git_branch_not_checked_out_fmt(&self, ahead: usize, behind: usize, base: &str) -> String {
        self.format(
            "git_branch_not_checked_out_fmt",
            &[
                ("ahead", &ahead.to_string()),
                ("behind", &behind.to_string()),
                ("base", base),
            ],
        )
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

    fn projects_delete_fmt(&self, path: &str) -> String {
        self.format("projects_delete_fmt", &[("path", path)])
    }

    fn app_quit_background_fmt(&self, projects: &str) -> String {
        self.format("app_quit_background_fmt", &[("projects", projects)])
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

    /// The keys this file reads through `get_string` and `format`, taken
    /// from its own source (the part above the tests).
    fn keys_read_by_source() -> (Vec<String>, Vec<String>) {
        let source = include_str!("runtime.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap()];
        let literals_after = |call: &str| -> Vec<String> {
            source
                .match_indices(call)
                .filter_map(|(at, _)| {
                    let rest = source[at + call.len()..].trim_start().strip_prefix('"')?;
                    Some(rest[..rest.find('"')?].to_string())
                })
                .collect()
        };
        let mut strings = literals_after("self.get_string(");
        let generated = source
            .split_once("i18n_get_string_methods! {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .unwrap()
            .0;
        strings.extend(
            generated
                .lines()
                .map(|line| line.split("//").next().unwrap())
                .flat_map(|line| line.split(','))
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string),
        );
        (strings, literals_after("self.format("))
    }

    /// A key read with `get_string` but kept in `[formats]` (or the other
    /// way round) comes out empty in every language.
    #[test]
    fn every_key_read_is_in_the_section_it_is_read_from() {
        let en = loader::load_language("en").unwrap();
        let (strings, formats) = keys_read_by_source();
        assert!(strings.len() > 300 && formats.len() > 100);
        let misplaced: Vec<_> = strings
            .iter()
            .filter(|key| !en.strings.contains_key(*key))
            .chain(formats.iter().filter(|key| !en.formats.contains_key(*key)))
            .collect();
        assert!(misplaced.is_empty(), "not in their section: {misplaced:?}");
    }

    /// The `{name}`s of a template.
    fn placeholders(template: &str) -> std::collections::BTreeSet<&str> {
        template
            .split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(name, _)| name))
            .collect()
    }

    /// A translation may leave out a placeholder (`{plural}` in a language
    /// without inflection) but not name one the code never fills.
    #[test]
    fn translations_use_only_the_placeholders_english_has() {
        let en = loader::load_language("en").unwrap();
        for (lang, _) in crate::SUPPORTED_LANGUAGES {
            let data = loader::load_language(lang).unwrap();
            for (key, template) in data.formats.iter().chain(&data.strings) {
                let Some(english) = en.formats.get(key).or_else(|| en.strings.get(key)) else {
                    panic!("{lang}: {key} is not in en.toml");
                };
                let extra: Vec<_> = placeholders(template)
                    .difference(&placeholders(english))
                    .copied()
                    .collect();
                assert!(extra.is_empty(), "{lang}: {key} has {extra:?}");
            }
        }
    }

    /// A key missing from a language silently shows the English text, so
    /// nothing at runtime points at it: every dictionary has exactly en's keys.
    #[test]
    fn every_language_has_every_key() {
        let en = loader::load_language("en").unwrap();
        for (lang, _) in crate::SUPPORTED_LANGUAGES {
            let data = loader::load_language(lang).unwrap();
            let mut missing: Vec<_> = en
                .strings
                .keys()
                .filter(|key| !data.strings.contains_key(*key))
                .chain(
                    en.formats
                        .keys()
                        .filter(|key| !data.formats.contains_key(*key)),
                )
                .collect();
            missing.sort();
            assert!(missing.is_empty(), "{lang} lacks {missing:?}");
            assert_eq!(
                data.strings.len(),
                en.strings.len(),
                "{lang}: extra strings"
            );
            assert_eq!(
                data.formats.len(),
                en.formats.len(),
                "{lang}: extra formats"
            );
        }
    }

    #[test]
    fn every_language_has_the_same_plural_words() {
        let en = loader::load_language("en").unwrap();
        let mut words: Vec<_> = en.plurals.keys().collect();
        words.sort();
        for (lang, _) in crate::SUPPORTED_LANGUAGES {
            let data = loader::load_language(lang).unwrap();
            let mut theirs: Vec<_> = data.plurals.keys().collect();
            theirs.sort();
            assert_eq!(theirs, words, "{lang}");
        }
    }

    #[test]
    fn russian_counts_take_the_form_their_last_digits_ask_for() {
        let t = RuntimeTranslation::new("ru").unwrap();
        let file = |n| format!("{n} файл{}", t.pluralize(n, "file"));
        assert_eq!(file(1), "1 файл");
        assert_eq!(file(3), "3 файла");
        assert_eq!(file(5), "5 файлов");
        assert_eq!(file(11), "11 файлов");
        assert_eq!(file(13), "13 файлов");
        assert_eq!(file(21), "21 файл");
        assert_eq!(file(22), "22 файла");
        assert_eq!(file(111), "111 файлов");
        assert_eq!(file(0), "0 файлов");
    }

    #[test]
    fn item_counts_are_pluralized() {
        let ru = RuntimeTranslation::new("ru").unwrap();
        assert_eq!(ru.status_item_count(3), "3 элемента");
        assert_eq!(ru.status_item_count(5), "5 элементов");
        assert_eq!(ru.status_item_count(21), "21 элемент");
        let en = RuntimeTranslation::new("en").unwrap();
        assert_eq!(en.status_item_count(2), "2 elements");
    }

    #[test]
    fn deleted_lines_are_counted_as_lines() {
        let de = RuntimeTranslation::new("de").unwrap();
        assert_eq!(de.editor_deletion_marker(1), "1 Zeile gelöscht");
        assert_eq!(de.editor_deletion_marker(3), "3 Zeilen gelöscht");
        let en = RuntimeTranslation::new("en").unwrap();
        assert_eq!(en.editor_deletion_marker(2), "2 lines deleted");
    }

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

            // Directory item count — a format key with a `{count}` placeholder,
            // so it must also resolve from `[formats]` in every language.
            let items = t.file_info_items(42);
            assert!(items.contains("42"), "{lang}: {items:?}");
        }
    }

    /// Every language must carry its own compact duration units — a missing key
    /// silently falls back to English, which is the bug these replaced.
    #[test]
    fn compact_duration_units_are_translated_everywhere() {
        let english = RuntimeTranslation::new("en").unwrap();
        let (en_h, en_m, en_s) = (
            english.time_short_hours().to_string(),
            english.time_short_minutes().to_string(),
            english.time_short_seconds().to_string(),
        );

        for (code, _) in crate::SUPPORTED_LANGUAGES {
            let t = RuntimeTranslation::new(code).unwrap();
            for unit in [
                t.time_short_hours(),
                t.time_short_minutes(),
                t.time_short_seconds(),
            ] {
                assert!(!unit.is_empty(), "{code}: missing a duration unit");
            }
            // Latin-script languages legitimately share some units with
            // English; the ones with their own script must not.
            if ["ru", "zh", "ja", "ko", "th", "hi", "bn"].contains(code) {
                assert_ne!(
                    (
                        t.time_short_hours(),
                        t.time_short_minutes(),
                        t.time_short_seconds()
                    ),
                    (en_h.as_str(), en_m.as_str(), en_s.as_str()),
                    "{code}: duration units left as the English fallback"
                );
            }
        }
    }

    /// The banner's field labels were literals, so every language showed the
    /// English keys. A dictionary that copies them over instead of translating
    /// them looks right to the key test above, so check the words themselves.
    #[test]
    fn banner_labels_are_translated_everywhere() {
        for (code, _) in crate::SUPPORTED_LANGUAGES {
            let t = RuntimeTranslation::new(code).unwrap();
            for label in [
                t.agent_banner_connection(),
                t.agent_banner_model(),
                t.agent_banner_tools(),
                t.agent_banner_cwd(),
                t.agent_banner_sessions(),
            ] {
                assert!(!label.is_empty(), "{code}: a banner label is empty");
            }
            // Latin-script languages legitimately share a word with English;
            // the ones with their own script must not. `cwd` is a path, so it
            // stays as is wherever the script allows.
            if ["ru", "zh", "ja", "ko", "th", "hi", "bn"].contains(code) {
                assert_ne!(t.agent_banner_connection(), "connection", "{code}");
                assert_ne!(t.agent_banner_model(), "model", "{code}");
                assert_ne!(t.agent_banner_tools(), "tools", "{code}");
                assert_ne!(t.agent_banner_sessions(), "sessions", "{code}");
            }
        }
    }

    /// The paste confirmation carries its verb inside each locale's own
    /// string. It used to take `{mode}` filled with a hardcoded `"Copy"`, so
    /// every non-English dictionary rendered the English word — and a verb
    /// slot cannot be filled grammatically where the verb goes last (de, ja,
    /// ko, tr, bn, hi). This guards both halves, and that the names block
    /// lands: the confirmation has to say *which* files, not only how many.
    #[test]
    fn paste_confirmation_carries_its_verb_in_every_language() {
        for (code, _) in crate::SUPPORTED_LANGUAGES {
            let t = RuntimeTranslation::new(code).unwrap();
            let msg = t.fm_paste_confirm(3, "a.txt\nb.txt", "/tmp/target");

            assert!(!msg.is_empty(), "{code}: empty paste confirmation");
            assert!(
                !msg.contains('{') && !msg.contains('}'),
                "{code}: unfilled placeholder in {msg:?}"
            );
            assert!(msg.contains("/tmp/target"), "{code}: lost the destination");
            assert!(msg.contains("a.txt"), "{code}: lost the names");

            // The verb must be localized, not the literal "Copy" the old call
            // site passed in. Latin-script languages legitimately share the
            // English word; scripted ones must not.
            if ["ru", "zh", "ja", "ko", "th", "hi", "bn"].contains(code) {
                assert!(
                    !msg.contains("Copy"),
                    "{code}: untranslated verb in {msg:?}"
                );
            }
        }
    }

    /// A paste of a cut deletes the sources, so its confirmation cannot read
    /// like a copy. Each move string swaps the verb in the locale's copy
    /// string, which is exactly the risk this guards: a swap that silently
    /// left the copy text in place would render "Copy" over a destructive Yes.
    #[test]
    fn the_move_confirmation_differs_from_the_copy_in_every_language() {
        for (code, _) in crate::SUPPORTED_LANGUAGES {
            let t = RuntimeTranslation::new(code).unwrap();
            let copy = t.fm_paste_confirm(3, "a.txt\nb.txt", "/tmp/target");
            let move_ = t.fm_paste_move_confirm(3, "a.txt\nb.txt", "/tmp/target");

            assert!(!move_.is_empty(), "{code}: empty move confirmation");
            assert!(
                !move_.contains('{') && !move_.contains('}'),
                "{code}: unfilled placeholder in {move_:?}"
            );
            assert!(
                move_.contains("/tmp/target"),
                "{code}: lost the destination"
            );
            assert!(move_.contains("a.txt"), "{code}: lost the names");
            assert_ne!(
                copy, move_,
                "{code}: a cut must not be confirmed with the copy wording"
            );

            if ["ru", "zh", "ja", "ko", "th", "hi", "bn"].contains(code) {
                assert!(
                    !move_.contains("Move") && !move_.contains("Copy"),
                    "{code}: untranslated verb in {move_:?}"
                );
            }
        }
    }

    /// A counted paste confirmation pluralizes where the language inflects:
    /// Russian asks for "файл / файла / файлов" by the last digits.
    #[test]
    fn paste_confirmation_pluralizes_per_language() {
        let ru = RuntimeTranslation::new("ru").unwrap();
        assert!(ru.fm_paste_confirm(1, "a", "/d").contains("файл"), "1");
        assert!(ru.fm_paste_confirm(3, "a", "/d").contains("файла"), "3");
        assert!(ru.fm_paste_confirm(7, "a", "/d").contains("файлов"), "7");

        let en = RuntimeTranslation::new("en").unwrap();
        assert!(
            en.fm_paste_confirm(1, "a", "/d").contains("file to"),
            "one: no plural"
        );
        assert!(
            en.fm_paste_confirm(2, "a", "/d").contains("files to"),
            "two: plural"
        );
    }
}
