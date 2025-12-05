use super::{loader, Translation};
use std::collections::HashMap;

/// Runtime translation implementation that loads from TOML files
pub struct RuntimeTranslation {
    strings: HashMap<String, String>,
    formats: HashMap<String, String>,
    plurals: HashMap<String, loader::PluralRules>,
}

impl RuntimeTranslation {
    pub fn new(lang: &str) -> anyhow::Result<Self> {
        let data = loader::load_language(lang)?;
        Ok(Self {
            strings: data.strings,
            formats: data.formats,
            plurals: data.plurals,
        })
    }

    fn get_string(&self, key: &str) -> &str {
        self.strings
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or_else(|| {
                eprintln!("Warning: Missing translation key: {}", key);
                ""
            })
    }

    fn format(&self, key: &str, args: &[(&str, &str)]) -> String {
        let template = self
            .formats
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or_else(|| {
                eprintln!("Warning: Missing format key: {}", key);
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
                2..=4 if rules.few.is_some() => rules.few.as_ref().unwrap(),
                _ => &rules.other,
            }
        } else if count == 1 {
            ""
        } else {
            "s"
        }
    }
}

impl Translation for RuntimeTranslation {
    fn fm_copy_files(&self) -> &str {
        self.get_string("fm_copy_files")
    }

    fn fm_cut_files(&self) -> &str {
        self.get_string("fm_cut_files")
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

    fn fm_delete_confirm(&self, count: usize) -> String {
        let plural = self.pluralize(count, "file");
        self.format(
            "fm_delete_confirm",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        self.format("fm_rename_prompt", &[("old_name", old_name)])
    }

    fn fm_create_file_prompt(&self) -> &str {
        self.get_string("fm_create_file_prompt")
    }

    fn fm_create_dir_prompt(&self) -> &str {
        self.get_string("fm_create_dir_prompt")
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        self.format("fm_copy_prompt", &[("name", name)])
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        self.format("fm_move_prompt", &[("name", name)])
    }

    fn fm_search_prompt(&self) -> &str {
        self.get_string("fm_search_prompt")
    }

    fn fm_no_results(&self) -> &str {
        self.get_string("fm_no_results")
    }

    fn fm_operation_cancelled(&self) -> &str {
        self.get_string("fm_operation_cancelled")
    }

    fn modal_yes(&self) -> &str {
        self.get_string("modal_yes")
    }

    fn modal_no(&self) -> &str {
        self.get_string("modal_no")
    }

    fn modal_ok(&self) -> &str {
        self.get_string("modal_ok")
    }

    fn modal_cancel(&self) -> &str {
        self.get_string("modal_cancel")
    }

    fn panel_file_manager(&self) -> &str {
        self.get_string("panel_file_manager")
    }

    fn panel_editor(&self, filename: &str) -> String {
        self.format("panel_editor", &[("filename", filename)])
    }

    fn panel_terminal(&self) -> &str {
        self.get_string("panel_terminal")
    }

    fn panel_welcome(&self) -> &str {
        self.get_string("panel_welcome")
    }

    fn editor_close_unsaved(&self) -> &str {
        self.get_string("editor_close_unsaved")
    }

    fn editor_close_unsaved_question(&self) -> &str {
        self.get_string("editor_close_unsaved_question")
    }

    fn editor_save_and_close(&self) -> &str {
        self.get_string("editor_save_and_close")
    }

    fn editor_close_without_saving(&self) -> &str {
        self.get_string("editor_close_without_saving")
    }

    fn editor_cancel(&self) -> &str {
        self.get_string("editor_cancel")
    }

    fn editor_save_error(&self, error: &str) -> String {
        self.format("editor_save_error", &[("error", error)])
    }

    fn editor_saved(&self, path: &str) -> String {
        self.format("editor_saved", &[("path", path)])
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        self.format("editor_file_opened", &[("filename", filename)])
    }

    fn editor_search_title(&self) -> &str {
        self.get_string("editor_search_title")
    }

    fn editor_search_prompt(&self) -> &str {
        self.get_string("editor_search_prompt")
    }

    fn editor_replace_title(&self) -> &str {
        self.get_string("editor_replace_title")
    }

    fn editor_replace_prompt(&self) -> &str {
        self.get_string("editor_replace_prompt")
    }

    fn editor_replace_with_prompt(&self) -> &str {
        self.get_string("editor_replace_with_prompt")
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

    fn editor_search_no_matches(&self) -> &str {
        self.get_string("editor_search_no_matches")
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        let plural = self.pluralize(count, "file");
        self.format(
            "editor_deletion_marker",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn terminal_exit_confirm(&self) -> &str {
        self.get_string("terminal_exit_confirm")
    }

    fn terminal_exited(&self, code: i32) -> String {
        self.format("terminal_exited", &[("code", &code.to_string())])
    }

    fn git_detected(&self) -> &str {
        self.get_string("git_detected")
    }

    fn git_not_found(&self) -> &str {
        self.get_string("git_not_found")
    }

    fn app_quit_confirm(&self) -> &str {
        self.get_string("app_quit_confirm")
    }

    fn error_operation_failed(&self, error: &str) -> String {
        self.format("error_operation_failed", &[("error", error)])
    }

    fn error_file_exists(&self, path: &str) -> String {
        self.format("error_file_exists", &[("path", path)])
    }

    fn error_invalid_path(&self) -> &str {
        self.get_string("error_invalid_path")
    }

    fn error_source_eq_dest(&self) -> &str {
        self.get_string("error_source_eq_dest")
    }

    fn error_dest_is_subdir(&self) -> &str {
        self.get_string("error_dest_is_subdir")
    }

    fn help_title(&self) -> &str {
        self.get_string("help_title")
    }

    fn help_app_title(&self) -> &str {
        self.get_string("help_app_title")
    }

    fn help_version(&self) -> &str {
        self.get_string("help_version")
    }

    fn help_global_keys(&self) -> &str {
        self.get_string("help_global_keys")
    }

    fn help_file_manager_keys(&self) -> &str {
        self.get_string("help_file_manager_keys")
    }

    fn help_editor_keys(&self) -> &str {
        self.get_string("help_editor_keys")
    }

    fn help_terminal_keys(&self) -> &str {
        self.get_string("help_terminal_keys")
    }

    fn help_git_integration(&self) -> &str {
        self.get_string("help_git_integration")
    }

    fn help_clipboard_operations(&self) -> &str {
        self.get_string("help_clipboard_operations")
    }

    fn help_close_hint(&self) -> &str {
        self.get_string("help_close_hint")
    }

    fn help_desc_menu(&self) -> &str {
        self.get_string("help_desc_menu")
    }

    fn help_desc_panel_menu(&self) -> &str {
        self.get_string("help_desc_panel_menu")
    }

    fn help_desc_quit(&self) -> &str {
        self.get_string("help_desc_quit")
    }

    fn help_desc_help(&self) -> &str {
        self.get_string("help_desc_help")
    }

    fn help_desc_switch_panel(&self) -> &str {
        self.get_string("help_desc_switch_panel")
    }

    fn help_desc_close_panel(&self) -> &str {
        self.get_string("help_desc_close_panel")
    }

    fn help_desc_navigate(&self) -> &str {
        self.get_string("help_desc_navigate")
    }

    fn help_desc_select(&self) -> &str {
        self.get_string("help_desc_select")
    }

    fn help_desc_select_all(&self) -> &str {
        self.get_string("help_desc_select_all")
    }

    fn help_desc_open_file(&self) -> &str {
        self.get_string("help_desc_open_file")
    }

    fn help_desc_open_editor(&self) -> &str {
        self.get_string("help_desc_open_editor")
    }

    fn help_desc_new_terminal(&self) -> &str {
        self.get_string("help_desc_new_terminal")
    }

    fn help_desc_parent_dir(&self) -> &str {
        self.get_string("help_desc_parent_dir")
    }

    fn help_desc_home(&self) -> &str {
        self.get_string("help_desc_home")
    }

    fn help_desc_end(&self) -> &str {
        self.get_string("help_desc_end")
    }

    fn help_desc_page_scroll(&self) -> &str {
        self.get_string("help_desc_page_scroll")
    }

    fn help_desc_create_file(&self) -> &str {
        self.get_string("help_desc_create_file")
    }

    fn help_desc_create_dir(&self) -> &str {
        self.get_string("help_desc_create_dir")
    }

    fn help_desc_rename(&self) -> &str {
        self.get_string("help_desc_rename")
    }

    fn help_desc_copy(&self) -> &str {
        self.get_string("help_desc_copy")
    }

    fn help_desc_move(&self) -> &str {
        self.get_string("help_desc_move")
    }

    fn help_desc_delete(&self) -> &str {
        self.get_string("help_desc_delete")
    }

    fn help_desc_search(&self) -> &str {
        self.get_string("help_desc_search")
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        self.get_string("help_desc_fm_copy_clipboard")
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        self.get_string("help_desc_fm_cut_clipboard")
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        self.get_string("help_desc_fm_paste_clipboard")
    }

    fn help_desc_save(&self) -> &str {
        self.get_string("help_desc_save")
    }

    fn help_desc_copy_system(&self) -> &str {
        self.get_string("help_desc_copy_system")
    }

    fn help_desc_paste_system(&self) -> &str {
        self.get_string("help_desc_paste_system")
    }

    fn help_desc_cut_system(&self) -> &str {
        self.get_string("help_desc_cut_system")
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        self.get_string("help_desc_paste_ctrl_y")
    }

    fn help_desc_undo(&self) -> &str {
        self.get_string("help_desc_undo")
    }

    fn help_desc_redo(&self) -> &str {
        self.get_string("help_desc_redo")
    }

    fn help_desc_git_ignored(&self) -> &str {
        self.get_string("help_desc_git_ignored")
    }

    fn help_desc_git_modified(&self) -> &str {
        self.get_string("help_desc_git_modified")
    }

    fn help_desc_git_added(&self) -> &str {
        self.get_string("help_desc_git_added")
    }

    fn help_desc_git_deleted(&self) -> &str {
        self.get_string("help_desc_git_deleted")
    }

    fn status_file_created(&self, name: &str) -> String {
        self.format("status_file_created", &[("name", name)])
    }

    fn status_error_create_file(&self, error: &str) -> String {
        self.format("status_error_create_file", &[("error", error)])
    }

    fn status_dir_created(&self, name: &str) -> String {
        self.format("status_dir_created", &[("name", name)])
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        self.format("status_error_create_dir", &[("error", error)])
    }

    fn status_item_deleted(&self) -> &str {
        self.get_string("status_item_deleted")
    }

    fn status_error_delete(&self) -> &str {
        self.get_string("status_error_delete")
    }

    fn status_items_deleted(&self, count: usize) -> String {
        let plural = self.pluralize(count, "file");
        self.format(
            "status_items_deleted",
            &[("count", &count.to_string()), ("plural", plural)],
        )
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        self.format(
            "status_items_deleted_with_errors",
            &[
                ("success", &success.to_string()),
                ("errors", &errors.to_string()),
            ],
        )
    }

    fn status_file_saved(&self, name: &str) -> String {
        self.format("status_file_saved", &[("name", name)])
    }

    fn status_error_save(&self, error: &str) -> String {
        self.format("status_error_save", &[("error", error)])
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        self.format(
            "status_error_open_file",
            &[("name", name), ("error", error)],
        )
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        self.format(
            "status_item_actioned",
            &[("name", name), ("action", action)],
        )
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        self.format(
            "status_error_action",
            &[("action", action), ("error", error)],
        )
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        self.format("status_operation_skipped", &[("name", name)])
    }

    fn action_copied(&self) -> &str {
        self.get_string("action_copied")
    }

    fn action_moved(&self) -> &str {
        self.get_string("action_moved")
    }

    fn action_copying(&self) -> &str {
        self.get_string("action_copying")
    }

    fn action_moving(&self) -> &str {
        self.get_string("action_moving")
    }

    fn modal_copy_single_title(&self, name: &str) -> String {
        self.format("modal_copy_single_title", &[("name", name)])
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        self.format(
            "modal_copy_multiple_title",
            &[("count", &count.to_string())],
        )
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        self.format("modal_move_single_title", &[("name", name)])
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        self.format(
            "modal_move_multiple_title",
            &[("count", &count.to_string())],
        )
    }

    fn modal_create_file_title(&self) -> &str {
        self.get_string("modal_create_file_title")
    }

    fn modal_create_dir_title(&self) -> &str {
        self.get_string("modal_create_dir_title")
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        self.format("modal_delete_single_title", &[("name", name)])
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        self.format(
            "modal_delete_multiple_title",
            &[("count", &count.to_string())],
        )
    }

    fn modal_save_as_title(&self) -> &str {
        self.get_string("modal_save_as_title")
    }

    fn modal_enter_filename(&self) -> &str {
        self.get_string("modal_enter_filename")
    }

    fn modal_copy_single_prompt(&self, name: &str) -> String {
        self.format("modal_copy_single_prompt", &[("name", name)])
    }

    fn modal_copy_multiple_prompt(&self, count: usize) -> String {
        self.format(
            "modal_copy_multiple_prompt",
            &[("count", &count.to_string())],
        )
    }

    fn modal_move_single_prompt(&self, name: &str) -> String {
        self.format("modal_move_single_prompt", &[("name", name)])
    }

    fn modal_move_multiple_prompt(&self, count: usize) -> String {
        self.format(
            "modal_move_multiple_prompt",
            &[("count", &count.to_string())],
        )
    }

    fn batch_result_file_copied(&self) -> &str {
        self.get_string("batch_result_file_copied")
    }

    fn batch_result_file_moved(&self) -> &str {
        self.get_string("batch_result_file_moved")
    }

    fn batch_result_error_copy(&self) -> &str {
        self.get_string("batch_result_error_copy")
    }

    fn batch_result_error_move(&self) -> &str {
        self.get_string("batch_result_error_move")
    }

    fn batch_result_copied(&self) -> &str {
        self.get_string("batch_result_copied")
    }

    fn batch_result_moved(&self) -> &str {
        self.get_string("batch_result_moved")
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        self.format("batch_result_skipped_fmt", &[("count", &count.to_string())])
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        self.format("batch_result_errors_fmt", &[("count", &count.to_string())])
    }

    fn menu_files(&self) -> &str {
        self.get_string("menu_files")
    }

    fn menu_terminal(&self) -> &str {
        self.get_string("menu_terminal")
    }

    fn menu_editor(&self) -> &str {
        self.get_string("menu_editor")
    }

    fn menu_debug(&self) -> &str {
        self.get_string("menu_debug")
    }

    fn menu_preferences(&self) -> &str {
        self.get_string("menu_preferences")
    }

    fn menu_help(&self) -> &str {
        self.get_string("menu_help")
    }

    fn menu_quit(&self) -> &str {
        self.get_string("menu_quit")
    }

    fn menu_navigate_hint(&self) -> &str {
        self.get_string("menu_navigate_hint")
    }

    fn menu_open_hint(&self) -> &str {
        self.get_string("menu_open_hint")
    }

    fn status_dir(&self) -> &str {
        self.get_string("status_dir")
    }

    fn status_file(&self) -> &str {
        self.get_string("status_file")
    }

    fn status_mod(&self) -> &str {
        self.get_string("status_mod")
    }

    fn status_owner(&self) -> &str {
        self.get_string("status_owner")
    }

    fn status_selected(&self) -> &str {
        self.get_string("status_selected")
    }

    fn status_pos(&self) -> &str {
        self.get_string("status_pos")
    }

    fn status_tab(&self) -> &str {
        self.get_string("status_tab")
    }

    fn status_plain_text(&self) -> &str {
        self.get_string("status_plain_text")
    }

    fn status_readonly(&self) -> &str {
        self.get_string("status_readonly")
    }

    fn status_cwd(&self) -> &str {
        self.get_string("status_cwd")
    }

    fn status_shell(&self) -> &str {
        self.get_string("status_shell")
    }

    fn status_terminal(&self) -> &str {
        self.get_string("status_terminal")
    }

    fn status_layout(&self) -> &str {
        self.get_string("status_layout")
    }

    fn status_panel(&self) -> &str {
        self.get_string("status_panel")
    }

    fn ui_yes(&self) -> &str {
        self.get_string("ui_yes")
    }

    fn ui_no(&self) -> &str {
        self.get_string("ui_no")
    }

    fn ui_ok(&self) -> &str {
        self.get_string("ui_ok")
    }

    fn ui_cancel(&self) -> &str {
        self.get_string("ui_cancel")
    }

    fn ui_continue(&self) -> &str {
        self.get_string("ui_continue")
    }

    fn ui_close(&self) -> &str {
        self.get_string("ui_close")
    }

    fn ui_enter_confirm(&self) -> &str {
        self.get_string("ui_enter_confirm")
    }

    fn ui_esc_cancel(&self) -> &str {
        self.get_string("ui_esc_cancel")
    }

    fn ui_hint_separator(&self) -> &str {
        self.get_string("ui_hint_separator")
    }

    fn size_bytes(&self) -> &str {
        self.get_string("size_bytes")
    }

    fn size_kilobytes(&self) -> &str {
        self.get_string("size_kilobytes")
    }

    fn size_megabytes(&self) -> &str {
        self.get_string("size_megabytes")
    }

    fn size_gigabytes(&self) -> &str {
        self.get_string("size_gigabytes")
    }

    fn size_terabytes(&self) -> &str {
        self.get_string("size_terabytes")
    }

    fn file_info_title(&self) -> &str {
        self.get_string("file_info_title")
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

    fn file_info_name(&self) -> &str {
        self.get_string("file_info_name")
    }

    fn file_info_path(&self) -> &str {
        self.get_string("file_info_path")
    }

    fn file_info_type(&self) -> &str {
        self.get_string("file_info_type")
    }

    fn file_info_size(&self) -> &str {
        self.get_string("file_info_size")
    }

    fn file_info_owner(&self) -> &str {
        self.get_string("file_info_owner")
    }

    fn file_info_group(&self) -> &str {
        self.get_string("file_info_group")
    }

    fn file_info_created(&self) -> &str {
        self.get_string("file_info_created")
    }

    fn file_info_modified(&self) -> &str {
        self.get_string("file_info_modified")
    }

    fn file_info_calculating(&self) -> &str {
        self.get_string("file_info_calculating")
    }

    fn file_info_press_key(&self) -> &str {
        self.get_string("file_info_press_key")
    }

    fn file_info_git(&self) -> &str {
        self.get_string("file_info_git")
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

    fn file_info_git_ignored(&self) -> &str {
        self.get_string("file_info_git_ignored")
    }

    fn file_type_directory(&self) -> &str {
        self.get_string("file_type_directory")
    }

    fn file_type_file(&self) -> &str {
        self.get_string("file_type_file")
    }

    fn file_type_symlink(&self) -> &str {
        self.get_string("file_type_symlink")
    }
}
