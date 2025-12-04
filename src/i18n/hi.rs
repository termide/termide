use super::Translation;

/// Hindi translation
/// हिन्दी अनुवाद
pub struct Hindi;

impl Translation for Hindi {
    // File Manager operations
    fn fm_copy_files(&self) -> &str {
        "फ़ाइलें क्लिपबोर्ड में कॉपी की गईं"
    }

    fn fm_cut_files(&self) -> &str {
        "फ़ाइलें क्लिपबोर्ड में कट की गईं"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        format!(
            "{} {} फ़ाइल{} यहाँ:\n{}",
            mode,
            count,
            if count == 1 { "" } else { "ें" },
            dest
        )
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!("{} फ़ाइल{} हटाएं?", count, if count == 1 { "" } else { "ें" })
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("'{}' का नाम बदलकर करें:", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "फ़ाइल का नाम दर्ज करें:"
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "डायरेक्टरी का नाम दर्ज करें:"
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("'{}' को यहाँ कॉपी करें:", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("'{}' को यहाँ ले जाएं:", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "खोजें:"
    }

    fn fm_no_results(&self) -> &str {
        "कोई मेल खाने वाली फ़ाइलें नहीं मिलीं"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "ऑपरेशन रद्द किया गया"
    }

    // Modal buttons
    fn modal_yes(&self) -> &str {
        "हाँ"
    }

    fn modal_no(&self) -> &str {
        "नहीं"
    }

    fn modal_ok(&self) -> &str {
        "ठीक है"
    }

    fn modal_cancel(&self) -> &str {
        "रद्द करें"
    }

    // Panel titles
    fn panel_file_manager(&self) -> &str {
        "फ़ाइल प्रबंधक"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("संपादक: {}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "टर्मिनल"
    }

    fn panel_welcome(&self) -> &str {
        "स्वागत है"
    }

    // Editor
    fn editor_close_unsaved(&self) -> &str {
        "संपादक बंद करें"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "फ़ाइल में असहेजे परिवर्तन हैं। क्या करें?"
    }

    fn editor_save_and_close(&self) -> &str {
        "सहेजें और बंद करें"
    }

    fn editor_close_without_saving(&self) -> &str {
        "बिना सहेजे बंद करें"
    }

    fn editor_cancel(&self) -> &str {
        "रद्द करें"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("फ़ाइल सहेजने में विफल: {}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("फ़ाइल सहेजी गई: {}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("फ़ाइल '{}' खोली गई", filename)
    }

    fn editor_search_title(&self) -> &str {
        "खोजें"
    }

    fn editor_search_prompt(&self) -> &str {
        "खोज क्वेरी दर्ज करें:"
    }

    fn editor_replace_title(&self) -> &str {
        "बदलें"
    }

    fn editor_replace_prompt(&self) -> &str {
        "यह खोजें:"
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "इससे बदलें:"
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("मिलान {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "कोई मिलान नहीं"
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        format!(
            "{} पंक्ति{} हटाई गई{}",
            count,
            if count == 1 { "" } else { "याँ" },
            if count == 1 { "" } else { "ं" }
        )
    }

    // Terminal
    fn terminal_exit_confirm(&self) -> &str {
        "प्रक्रिया अभी चल रही है। टर्मिनल बंद करें?"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("प्रक्रिया कोड {} के साथ समाप्त हुई", code)
    }

    // Git status
    fn git_detected(&self) -> &str {
        "Git मिला और उपलब्ध है"
    }

    fn git_not_found(&self) -> &str {
        "Git नहीं मिला - git एकीकरण अक्षम"
    }

    fn app_quit_confirm(&self) -> &str {
        "असहेजे परिवर्तन हैं। फिर भी बाहर निकलें?"
    }

    // Errors
    fn error_operation_failed(&self, error: &str) -> String {
        format!("ऑपरेशन विफल: {}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("फ़ाइल या डायरेक्टरी पहले से मौजूद है: {}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "अमान्य पथ"
    }

    fn error_source_eq_dest(&self) -> &str {
        "स्रोत और गंतव्य समान हैं"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "गंतव्य स्रोत की उपनिर्देशिका है"
    }

    // Help modal
    fn help_title(&self) -> &str {
        "सहायता"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - सहायता"
    }

    fn help_version(&self) -> &str {
        "0.3.0"
    }

    fn help_global_keys(&self) -> &str {
        "वैश्विक हॉटकीज़"
    }

    fn help_file_manager_keys(&self) -> &str {
        "फ़ाइल प्रबंधक"
    }

    fn help_editor_keys(&self) -> &str {
        "टेक्स्ट संपादक"
    }

    fn help_terminal_keys(&self) -> &str {
        "टर्मिनल"
    }

    fn help_git_integration(&self) -> &str {
        "GIT एकीकरण"
    }

    fn help_clipboard_operations(&self) -> &str {
        "क्लिपबोर्ड संचालन"
    }

    fn help_close_hint(&self) -> &str {
        "इस विंडो को बंद करने के लिए Esc या Ctrl+H दबाएं"
    }

    // Help key descriptions - Global
    fn help_desc_menu(&self) -> &str {
        "मेनू खोलें/बंद करें"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "पैनल मेनू"
    }

    fn help_desc_quit(&self) -> &str {
        "एप्लिकेशन बंद करें"
    }

    fn help_desc_help(&self) -> &str {
        "सहायता दिखाएं"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "पैनलों के बीच स्विच करें"
    }

    fn help_desc_close_panel(&self) -> &str {
        "सक्रिय पैनल बंद करें"
    }

    // Help key descriptions - File Manager
    fn help_desc_navigate(&self) -> &str {
        "सूची में नेविगेट करें"
    }

    fn help_desc_select(&self) -> &str {
        "चयन टॉगल करें"
    }

    fn help_desc_select_all(&self) -> &str {
        "सभी फ़ाइलें चुनें"
    }

    fn help_desc_open_file(&self) -> &str {
        "संपादक में डायरेक्टरी या फ़ाइल खोलें"
    }

    fn help_desc_open_editor(&self) -> &str {
        "संपादक में फ़ाइल खोलें"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "वर्तमान डायरेक्टरी में नया टर्मिनल खोलें"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "मूल डायरेक्टरी पर जाएं"
    }

    fn help_desc_home(&self) -> &str {
        "सूची की शुरुआत में जाएं"
    }

    fn help_desc_end(&self) -> &str {
        "सूची के अंत में जाएं"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "पृष्ठों द्वारा स्क्रॉल करें"
    }

    fn help_desc_create_file(&self) -> &str {
        "नई फ़ाइल बनाएं"
    }

    fn help_desc_create_dir(&self) -> &str {
        "नई डायरेक्टरी बनाएं"
    }

    fn help_desc_rename(&self) -> &str {
        "फ़ाइल/डायरेक्टरी का नाम बदलें"
    }

    fn help_desc_copy(&self) -> &str {
        "फ़ाइल/डायरेक्टरी कॉपी करें"
    }

    fn help_desc_move(&self) -> &str {
        "फ़ाइल/डायरेक्टरी ले जाएं"
    }

    fn help_desc_delete(&self) -> &str {
        "चयनित फ़ाइल/डायरेक्टरी हटाएं"
    }

    fn help_desc_search(&self) -> &str {
        "फ़ाइलें खोजें"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "चयनित फ़ाइलें फ़ाइल क्लिपबोर्ड में कॉपी करें"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "चयनित फ़ाइलें फ़ाइल क्लिपबोर्ड में कट करें"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "फ़ाइल क्लिपबोर्ड से फ़ाइलें पेस्ट करें"
    }

    // Help key descriptions - Editor
    fn help_desc_save(&self) -> &str {
        "फ़ाइल सहेजें"
    }

    fn help_desc_copy_system(&self) -> &str {
        "सिस्टम क्लिपबोर्ड में कॉपी करें"
    }

    fn help_desc_paste_system(&self) -> &str {
        "सिस्टम क्लिपबोर्ड से पेस्ट करें"
    }

    fn help_desc_cut_system(&self) -> &str {
        "सिस्टम क्लिपबोर्ड में कट करें"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "सिस्टम क्लिपबोर्ड से पेस्ट करें"
    }

    fn help_desc_undo(&self) -> &str {
        "पूर्ववत करें"
    }

    fn help_desc_redo(&self) -> &str {
        "फिर से करें"
    }

    // Help key descriptions - Git colors
    fn help_desc_git_ignored(&self) -> &str {
        "गहरा भूरा - git द्वारा नज़रअंदाज़"
    }

    fn help_desc_git_modified(&self) -> &str {
        "पीला - संशोधित"
    }

    fn help_desc_git_added(&self) -> &str {
        "हरा - नया/जोड़ा गया"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "हल्का लाल - हटाया गया (केवल पढ़ने के लिए)"
    }

    // File operation status messages
    fn status_file_created(&self, name: &str) -> String {
        format!("फ़ाइल '{}' बनाई गई", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("फ़ाइल बनाने में त्रुटि: {}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("डायरेक्टरी '{}' बनाई गई", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("डायरेक्टरी बनाने में त्रुटि: {}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "आइटम हटाया गया"
    }

    fn status_error_delete(&self) -> &str {
        "हटाने में त्रुटि"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("{} आइटम हटाए गए", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("हटाए गए: {}, त्रुटियां: {}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("फ़ाइल '{}' सहेजी गई", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("सहेजने में त्रुटि: {}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("'{}' खोलने में त्रुटि: {}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("{} में त्रुटि: {}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("ऑपरेशन '{}' छोड़ा गया", name)
    }

    // Action words
    fn action_copied(&self) -> &str {
        "कॉपी किया गया"
    }

    fn action_moved(&self) -> &str {
        "ले जाया गया"
    }

    fn action_copying(&self) -> &str {
        "कॉपी किया जा रहा है"
    }

    fn action_moving(&self) -> &str {
        "ले जाया जा रहा है"
    }

    // Modal titles and prompts for copy/move
    fn modal_copy_single_title(&self, name: &str) -> String {
        format!("'{}' कॉपी करें", name)
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        format!("{} तत्व कॉपी करें", count)
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        format!("'{}' ले जाएं", name)
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        format!("{} तत्व ले जाएं", count)
    }

    fn modal_create_file_title(&self) -> &str {
        "फ़ाइल बनाएं"
    }

    fn modal_create_dir_title(&self) -> &str {
        "डायरेक्टरी बनाएं"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("'{}' हटाएं", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("{} तत्व हटाएं", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "इस रूप में सहेजें"
    }

    fn modal_enter_filename(&self) -> &str {
        "फ़ाइल का नाम दर्ज करें:"
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
        "कॉपी किया गया"
    }

    fn batch_result_file_moved(&self) -> &str {
        "ले जाया गया"
    }

    fn batch_result_error_copy(&self) -> &str {
        "कॉपी करने में त्रुटि"
    }

    fn batch_result_error_move(&self) -> &str {
        "ले जाने में त्रुटि"
    }

    fn batch_result_copied(&self) -> &str {
        "कॉपी किया गया"
    }

    fn batch_result_moved(&self) -> &str {
        "ले जाया गया"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("छोड़ा गया: {}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("त्रुटियां: {}", count)
    }

    // Menu items
    fn menu_files(&self) -> &str {
        "फ़ाइलें"
    }

    fn menu_terminal(&self) -> &str {
        "टर्मिनल"
    }

    fn menu_editor(&self) -> &str {
        "संपादक"
    }

    fn menu_debug(&self) -> &str {
        "लॉग"
    }

    fn menu_preferences(&self) -> &str {
        "प्राथमिकताएं"
    }

    fn menu_help(&self) -> &str {
        "सहायता"
    }

    fn menu_quit(&self) -> &str {
        "बाहर निकलें"
    }

    // Menu hints
    fn menu_navigate_hint(&self) -> &str {
        "←→ नेविगेट | Enter चुनें | Esc बंद करें"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M मेनू"
    }

    // Status bar labels
    fn status_dir(&self) -> &str {
        "डायर:"
    }

    fn status_file(&self) -> &str {
        "फ़ाइल:"
    }

    fn status_mod(&self) -> &str {
        "मॉड:"
    }

    fn status_owner(&self) -> &str {
        "स्वामी:"
    }

    fn status_selected(&self) -> &str {
        "चयनित:"
    }

    fn status_pos(&self) -> &str {
        "स्थिति:"
    }

    fn status_tab(&self) -> &str {
        "टैब:"
    }

    fn status_plain_text(&self) -> &str {
        "सादा टेक्स्ट"
    }

    fn status_readonly(&self) -> &str {
        "[RO]"
    }

    fn status_cwd(&self) -> &str {
        "CWD:"
    }

    fn status_shell(&self) -> &str {
        "शेल:"
    }

    fn status_terminal(&self) -> &str {
        "टर्मिनल:"
    }

    fn status_layout(&self) -> &str {
        "लेआउट:"
    }

    fn status_panel(&self) -> &str {
        "पैनल:"
    }

    // UI elements
    fn ui_yes(&self) -> &str {
        "हाँ"
    }

    fn ui_no(&self) -> &str {
        "नहीं"
    }

    fn ui_ok(&self) -> &str {
        "ठीक है"
    }

    fn ui_cancel(&self) -> &str {
        "रद्द करें"
    }

    fn ui_continue(&self) -> &str {
        "जारी रखें"
    }

    fn ui_close(&self) -> &str {
        "बंद करें"
    }

    fn ui_enter_confirm(&self) -> &str {
        "Enter - पुष्टि करें"
    }

    fn ui_esc_cancel(&self) -> &str {
        "Esc - रद्द करें"
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
        "फ़ाइल जानकारी"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("फ़ाइल जानकारी '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("डायरेक्टरी जानकारी '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("सिमलिंक जानकारी '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "नाम"
    }

    fn file_info_path(&self) -> &str {
        "पथ"
    }

    fn file_info_type(&self) -> &str {
        "प्रकार"
    }

    fn file_info_size(&self) -> &str {
        "आकार"
    }

    fn file_info_owner(&self) -> &str {
        "स्वामी"
    }

    fn file_info_group(&self) -> &str {
        "समूह"
    }

    fn file_info_created(&self) -> &str {
        "बनाया गया"
    }

    fn file_info_modified(&self) -> &str {
        "संशोधित"
    }

    fn file_info_calculating(&self) -> &str {
        "गणना की जा रही है"
    }

    fn file_info_press_key(&self) -> &str {
        "बंद करने के लिए कोई भी कुंजी दबाएं"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        if count == 0 {
            "कमिट करने के लिए कोई परिवर्तन नहीं".to_string()
        } else if count == 1 {
            "कमिट करने के लिए 1 परिवर्तन".to_string()
        } else {
            format!("कमिट करने के लिए {} परिवर्तन", count)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        if count == 0 {
            "पुश करने के लिए कोई कमिट नहीं".to_string()
        } else if count == 1 {
            "पुश करने के लिए 1 कमिट".to_string()
        } else {
            format!("पुश करने के लिए {} कमिट", count)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        if count == 0 {
            "पुल करने के लिए कोई कमिट नहीं".to_string()
        } else if count == 1 {
            "पुल करने के लिए 1 कमिट".to_string()
        } else {
            format!("पुल करने के लिए {} कमिट", count)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "git इंडेक्स में नहीं"
    }

    // File types
    fn file_type_directory(&self) -> &str {
        "डायरेक्टरी"
    }

    fn file_type_file(&self) -> &str {
        "फ़ाइल"
    }

    fn file_type_symlink(&self) -> &str {
        "सिमलिंक"
    }
}
