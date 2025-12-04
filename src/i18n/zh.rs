use super::Translation;

/// Chinese (Simplified) translation
/// 简体中文翻译
pub struct Chinese;

impl Translation for Chinese {
    // File Manager operations
    fn fm_copy_files(&self) -> &str {
        "文件已复制到剪贴板"
    }

    fn fm_cut_files(&self) -> &str {
        "文件已剪切到剪贴板"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        format!("{} {} 个文件到：\n{}", mode, count, dest)
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!("删除 {} 个文件？", count)
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("将 '{}' 重命名为：", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "输入文件名："
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "输入目录名："
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("复制 '{}' 到：", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("移动 '{}' 到：", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "搜索："
    }

    fn fm_no_results(&self) -> &str {
        "未找到匹配的文件"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "操作已取消"
    }

    // Modal buttons
    fn modal_yes(&self) -> &str {
        "是"
    }

    fn modal_no(&self) -> &str {
        "否"
    }

    fn modal_ok(&self) -> &str {
        "确定"
    }

    fn modal_cancel(&self) -> &str {
        "取消"
    }

    // Panel titles
    fn panel_file_manager(&self) -> &str {
        "文件管理器"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("编辑器：{}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "终端"
    }

    fn panel_welcome(&self) -> &str {
        "欢迎"
    }

    // Editor
    fn editor_close_unsaved(&self) -> &str {
        "关闭编辑器"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "文件有未保存的更改。如何处理？"
    }

    fn editor_save_and_close(&self) -> &str {
        "保存并关闭"
    }

    fn editor_close_without_saving(&self) -> &str {
        "不保存并关闭"
    }

    fn editor_cancel(&self) -> &str {
        "取消"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("文件保存失败：{}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("文件已保存：{}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("文件 '{}' 已打开", filename)
    }

    fn editor_search_title(&self) -> &str {
        "搜索"
    }

    fn editor_search_prompt(&self) -> &str {
        "输入搜索内容："
    }

    fn editor_replace_title(&self) -> &str {
        "替换"
    }

    fn editor_replace_prompt(&self) -> &str {
        "查找："
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "替换为："
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("匹配 {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "无匹配项"
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        format!("已删除 {} 行", count)
    }

    // Terminal
    fn terminal_exit_confirm(&self) -> &str {
        "进程仍在运行。关闭终端？"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("进程已退出，代码 {}", code)
    }

    // Git status
    fn git_detected(&self) -> &str {
        "检测到 Git 且可用"
    }

    fn git_not_found(&self) -> &str {
        "未找到 Git - git 集成已禁用"
    }

    fn app_quit_confirm(&self) -> &str {
        "有未保存的更改。仍要退出吗？"
    }

    // Errors
    fn error_operation_failed(&self, error: &str) -> String {
        format!("操作失败：{}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("文件或目录已存在：{}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "路径无效"
    }

    fn error_source_eq_dest(&self) -> &str {
        "源和目标相同"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "目标是源的子目录"
    }

    // Help modal
    fn help_title(&self) -> &str {
        "帮助"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - 帮助"
    }

    fn help_version(&self) -> &str {
        "0.3.0"
    }

    fn help_global_keys(&self) -> &str {
        "全局快捷键"
    }

    fn help_file_manager_keys(&self) -> &str {
        "文件管理器"
    }

    fn help_editor_keys(&self) -> &str {
        "文本编辑器"
    }

    fn help_terminal_keys(&self) -> &str {
        "终端"
    }

    fn help_git_integration(&self) -> &str {
        "GIT 集成"
    }

    fn help_clipboard_operations(&self) -> &str {
        "剪贴板操作"
    }

    fn help_close_hint(&self) -> &str {
        "按 Esc 或 Ctrl+H 关闭此窗口"
    }

    // Help key descriptions - Global
    fn help_desc_menu(&self) -> &str {
        "打开/关闭菜单"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "面板菜单"
    }

    fn help_desc_quit(&self) -> &str {
        "退出应用程序"
    }

    fn help_desc_help(&self) -> &str {
        "显示帮助"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "在面板之间切换"
    }

    fn help_desc_close_panel(&self) -> &str {
        "关闭活动面板"
    }

    // Help key descriptions - File Manager
    fn help_desc_navigate(&self) -> &str {
        "浏览列表"
    }

    fn help_desc_select(&self) -> &str {
        "切换选择"
    }

    fn help_desc_select_all(&self) -> &str {
        "全选文件"
    }

    fn help_desc_open_file(&self) -> &str {
        "在编辑器中打开目录或文件"
    }

    fn help_desc_open_editor(&self) -> &str {
        "在编辑器中打开文件"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "在当前目录打开新终端"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "转到上级目录"
    }

    fn help_desc_home(&self) -> &str {
        "转到列表开头"
    }

    fn help_desc_end(&self) -> &str {
        "转到列表末尾"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "按页滚动"
    }

    fn help_desc_create_file(&self) -> &str {
        "创建新文件"
    }

    fn help_desc_create_dir(&self) -> &str {
        "创建新目录"
    }

    fn help_desc_rename(&self) -> &str {
        "重命名文件/目录"
    }

    fn help_desc_copy(&self) -> &str {
        "复制文件/目录"
    }

    fn help_desc_move(&self) -> &str {
        "移动文件/目录"
    }

    fn help_desc_delete(&self) -> &str {
        "删除所选文件/目录"
    }

    fn help_desc_search(&self) -> &str {
        "搜索文件"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "将所选文件复制到文件剪贴板"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "将所选文件剪切到文件剪贴板"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "从文件剪贴板粘贴文件"
    }

    // Help key descriptions - Editor
    fn help_desc_save(&self) -> &str {
        "保存文件"
    }

    fn help_desc_copy_system(&self) -> &str {
        "复制到系统剪贴板"
    }

    fn help_desc_paste_system(&self) -> &str {
        "从系统剪贴板粘贴"
    }

    fn help_desc_cut_system(&self) -> &str {
        "剪切到系统剪贴板"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "从系统剪贴板粘贴"
    }

    fn help_desc_undo(&self) -> &str {
        "撤销"
    }

    fn help_desc_redo(&self) -> &str {
        "重做"
    }

    // Help key descriptions - Git colors
    fn help_desc_git_ignored(&self) -> &str {
        "深灰色 - git 忽略"
    }

    fn help_desc_git_modified(&self) -> &str {
        "黄色 - 已修改"
    }

    fn help_desc_git_added(&self) -> &str {
        "绿色 - 新增/已添加"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "淡红色 - 已删除（只读）"
    }

    // File operation status messages
    fn status_file_created(&self, name: &str) -> String {
        format!("文件 '{}' 已创建", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("创建文件错误：{}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("目录 '{}' 已创建", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("创建目录错误：{}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "项目已删除"
    }

    fn status_error_delete(&self) -> &str {
        "删除错误"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("已删除 {} 个项目", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("已删除：{}，错误：{}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("文件 '{}' 已保存", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("保存错误：{}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("打开 '{}' 错误：{}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("错误 {}：{}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("操作 '{}' 已跳过", name)
    }

    // Action words
    fn action_copied(&self) -> &str {
        "已复制"
    }

    fn action_moved(&self) -> &str {
        "已移动"
    }

    fn action_copying(&self) -> &str {
        "复制中"
    }

    fn action_moving(&self) -> &str {
        "移动中"
    }

    // Modal titles and prompts for copy/move
    fn modal_copy_single_title(&self, name: &str) -> String {
        format!("复制 '{}'", name)
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        format!("复制 {} 个元素", count)
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        format!("移动 '{}'", name)
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        format!("移动 {} 个元素", count)
    }

    fn modal_create_file_title(&self) -> &str {
        "创建文件"
    }

    fn modal_create_dir_title(&self) -> &str {
        "创建目录"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("删除 '{}'", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("删除 {} 个元素", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "另存为"
    }

    fn modal_enter_filename(&self) -> &str {
        "输入文件名："
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
        "已复制"
    }

    fn batch_result_file_moved(&self) -> &str {
        "已移动"
    }

    fn batch_result_error_copy(&self) -> &str {
        "复制错误"
    }

    fn batch_result_error_move(&self) -> &str {
        "移动错误"
    }

    fn batch_result_copied(&self) -> &str {
        "已复制"
    }

    fn batch_result_moved(&self) -> &str {
        "已移动"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("已跳过：{}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("错误：{}", count)
    }

    // Menu items
    fn menu_files(&self) -> &str {
        "文件"
    }

    fn menu_terminal(&self) -> &str {
        "终端"
    }

    fn menu_editor(&self) -> &str {
        "编辑器"
    }

    fn menu_debug(&self) -> &str {
        "日志"
    }

    fn menu_preferences(&self) -> &str {
        "偏好设置"
    }

    fn menu_help(&self) -> &str {
        "帮助"
    }

    fn menu_quit(&self) -> &str {
        "退出"
    }

    // Menu hints
    fn menu_navigate_hint(&self) -> &str {
        "←→ 导航 | Enter 选择 | Esc 关闭"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M 菜单"
    }

    // Status bar labels
    fn status_dir(&self) -> &str {
        "目录："
    }

    fn status_file(&self) -> &str {
        "文件："
    }

    fn status_mod(&self) -> &str {
        "修改："
    }

    fn status_owner(&self) -> &str {
        "所有者："
    }

    fn status_selected(&self) -> &str {
        "已选："
    }

    fn status_pos(&self) -> &str {
        "位置："
    }

    fn status_tab(&self) -> &str {
        "制表符："
    }

    fn status_plain_text(&self) -> &str {
        "纯文本"
    }

    fn status_readonly(&self) -> &str {
        "[只读]"
    }

    fn status_cwd(&self) -> &str {
        "当前目录："
    }

    fn status_shell(&self) -> &str {
        "Shell："
    }

    fn status_terminal(&self) -> &str {
        "终端："
    }

    fn status_layout(&self) -> &str {
        "布局："
    }

    fn status_panel(&self) -> &str {
        "面板："
    }

    // UI elements
    fn ui_yes(&self) -> &str {
        "是"
    }

    fn ui_no(&self) -> &str {
        "否"
    }

    fn ui_ok(&self) -> &str {
        "确定"
    }

    fn ui_cancel(&self) -> &str {
        "取消"
    }

    fn ui_continue(&self) -> &str {
        "继续"
    }

    fn ui_close(&self) -> &str {
        "关闭"
    }

    fn ui_enter_confirm(&self) -> &str {
        "Enter - 确认"
    }

    fn ui_esc_cancel(&self) -> &str {
        "Esc - 取消"
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
        "文件信息"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("文件信息 '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("目录信息 '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("符号链接信息 '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "名称"
    }

    fn file_info_path(&self) -> &str {
        "路径"
    }

    fn file_info_type(&self) -> &str {
        "类型"
    }

    fn file_info_size(&self) -> &str {
        "大小"
    }

    fn file_info_owner(&self) -> &str {
        "所有者"
    }

    fn file_info_group(&self) -> &str {
        "组"
    }

    fn file_info_created(&self) -> &str {
        "创建时间"
    }

    fn file_info_modified(&self) -> &str {
        "修改时间"
    }

    fn file_info_calculating(&self) -> &str {
        "计算中"
    }

    fn file_info_press_key(&self) -> &str {
        "按任意键关闭"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        if count == 0 {
            "无需提交的更改".to_string()
        } else {
            format!("{} 个更改待提交", count)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        if count == 0 {
            "无需推送的提交".to_string()
        } else {
            format!("{} 个提交待推送", count)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        if count == 0 {
            "无需拉取的提交".to_string()
        } else {
            format!("{} 个提交待拉取", count)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "不在 git 索引中"
    }

    // File types
    fn file_type_directory(&self) -> &str {
        "目录"
    }

    fn file_type_file(&self) -> &str {
        "文件"
    }

    fn file_type_symlink(&self) -> &str {
        "符号链接"
    }
}
