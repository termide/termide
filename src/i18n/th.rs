use super::Translation;

/// Thai translation
/// การแปลภาษาไทย
pub struct Thai;

impl Translation for Thai {
    fn fm_copy_files(&self) -> &str {
        "คัดลอกไฟล์ไปยังคลิปบอร์ดแล้ว"
    }

    fn fm_cut_files(&self) -> &str {
        "ตัดไฟล์ไปยังคลิปบอร์ดแล้ว"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        format!("{} {} ไฟล์ไปยัง:\n{}", mode, count, dest)
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!("ลบ {} ไฟล์?", count)
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("เปลี่ยนชื่อ '{}' เป็น:", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "ป้อนชื่อไฟล์:"
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "ป้อนชื่อไดเรกทอรี:"
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("คัดลอก '{}' ไปยัง:", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("ย้าย '{}' ไปยัง:", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "ค้นหา:"
    }

    fn fm_no_results(&self) -> &str {
        "ไม่พบไฟล์ที่ตรงกัน"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "ยกเลิกการดำเนินการแล้ว"
    }

    fn modal_yes(&self) -> &str {
        "ใช่"
    }

    fn modal_no(&self) -> &str {
        "ไม่"
    }

    fn modal_ok(&self) -> &str {
        "ตกลง"
    }

    fn modal_cancel(&self) -> &str {
        "ยกเลิก"
    }

    fn panel_file_manager(&self) -> &str {
        "ตัวจัดการไฟล์"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("ตัวแก้ไข: {}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "เทอร์มินัล"
    }

    fn panel_welcome(&self) -> &str {
        "ยินดีต้อนรับ"
    }

    fn editor_close_unsaved(&self) -> &str {
        "ปิดตัวแก้ไข"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "ไฟล์มีการเปลี่ยนแปลงที่ยังไม่บันทึก จะทำอย่างไร?"
    }

    fn editor_save_and_close(&self) -> &str {
        "บันทึกและปิด"
    }

    fn editor_close_without_saving(&self) -> &str {
        "ปิดโดยไม่บันทึก"
    }

    fn editor_cancel(&self) -> &str {
        "ยกเลิก"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("ไม่สามารถบันทึกไฟล์: {}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("บันทึกไฟล์แล้ว: {}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("เปิดไฟล์ '{}' แล้ว", filename)
    }

    fn editor_search_title(&self) -> &str {
        "ค้นหา"
    }

    fn editor_search_prompt(&self) -> &str {
        "ป้อนคำค้นหา:"
    }

    fn editor_replace_title(&self) -> &str {
        "แทนที่"
    }

    fn editor_replace_prompt(&self) -> &str {
        "ค้นหา:"
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "แทนที่ด้วย:"
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("ผลลัพธ์ที่ {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "ไม่พบผลลัพธ์"
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        format!("ลบ {} บรรทัดแล้ว", count)
    }

    fn terminal_exit_confirm(&self) -> &str {
        "โปรเซสยังทำงานอยู่ ปิดเทอร์มินัล?"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("โปรเซสสิ้นสุดด้วยรหัส {}", code)
    }

    fn git_detected(&self) -> &str {
        "ตรวจพบ Git และพร้อมใช้งาน"
    }

    fn git_not_found(&self) -> &str {
        "ไม่พบ Git - ปิดการใช้งานการผสานรวม git"
    }

    fn app_quit_confirm(&self) -> &str {
        "มีการเปลี่ยนแปลงที่ยังไม่บันทึก ออกจากโปรแกรมหรือไม่?"
    }

    fn error_operation_failed(&self, error: &str) -> String {
        format!("การดำเนินการล้มเหลว: {}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("ไฟล์หรือไดเรกทอรีมีอยู่แล้ว: {}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "เส้นทางไม่ถูกต้อง"
    }

    fn error_source_eq_dest(&self) -> &str {
        "ต้นทางและปลายทางเหมือนกัน"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "ปลายทางเป็นไดเรกทอรีย่อยของต้นทาง"
    }

    fn help_title(&self) -> &str {
        "ช่วยเหลือ"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - ช่วยเหลือ"
    }

    fn help_version(&self) -> &str {
        "0.3.0"
    }

    fn help_global_keys(&self) -> &str {
        "ปุ่มลัดทั่วไป"
    }

    fn help_file_manager_keys(&self) -> &str {
        "ตัวจัดการไฟล์"
    }

    fn help_editor_keys(&self) -> &str {
        "ตัวแก้ไขข้อความ"
    }

    fn help_terminal_keys(&self) -> &str {
        "เทอร์มินัล"
    }

    fn help_git_integration(&self) -> &str {
        "การผสานรวม GIT"
    }

    fn help_clipboard_operations(&self) -> &str {
        "การดำเนินการคลิปบอร์ด"
    }

    fn help_close_hint(&self) -> &str {
        "กด Esc หรือ Ctrl+H เพื่อปิดหน้าต่างนี้"
    }

    fn help_desc_menu(&self) -> &str {
        "เปิด/ปิดเมนู"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "เมนูแผง"
    }

    fn help_desc_quit(&self) -> &str {
        "ออกจากแอปพลิเคชัน"
    }

    fn help_desc_help(&self) -> &str {
        "แสดงความช่วยเหลือ"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "สลับระหว่างแผง"
    }

    fn help_desc_close_panel(&self) -> &str {
        "ปิดแผงที่ใช้งานอยู่"
    }

    fn help_desc_navigate(&self) -> &str {
        "เลื่อนดูรายการ"
    }

    fn help_desc_select(&self) -> &str {
        "สลับการเลือก"
    }

    fn help_desc_select_all(&self) -> &str {
        "เลือกไฟล์ทั้งหมด"
    }

    fn help_desc_open_file(&self) -> &str {
        "เปิดไดเรกทอรีหรือไฟล์ในตัวแก้ไข"
    }

    fn help_desc_open_editor(&self) -> &str {
        "เปิดไฟล์ในตัวแก้ไข"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "เปิดเทอร์มินัลใหม่ในไดเรกทอรีปัจจุบัน"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "ไปยังไดเรกทอรีแม่"
    }

    fn help_desc_home(&self) -> &str {
        "ไปยังต้นรายการ"
    }

    fn help_desc_end(&self) -> &str {
        "ไปยังท้ายรายการ"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "เลื่อนทีละหน้า"
    }

    fn help_desc_create_file(&self) -> &str {
        "สร้างไฟล์ใหม่"
    }

    fn help_desc_create_dir(&self) -> &str {
        "สร้างไดเรกทอรีใหม่"
    }

    fn help_desc_rename(&self) -> &str {
        "เปลี่ยนชื่อไฟล์/ไดเรกทอรี"
    }

    fn help_desc_copy(&self) -> &str {
        "คัดลอกไฟล์/ไดเรกทอรี"
    }

    fn help_desc_move(&self) -> &str {
        "ย้ายไฟล์/ไดเรกทอรี"
    }

    fn help_desc_delete(&self) -> &str {
        "ลบไฟล์/ไดเรกทอรีที่เลือก"
    }

    fn help_desc_search(&self) -> &str {
        "ค้นหาไฟล์"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "คัดลอกไฟล์ที่เลือกไปยังคลิปบอร์ด"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "ตัดไฟล์ที่เลือกไปยังคลิปบอร์ด"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "วางไฟล์จากคลิปบอร์ด"
    }

    fn help_desc_save(&self) -> &str {
        "บันทึกไฟล์"
    }

    fn help_desc_copy_system(&self) -> &str {
        "คัดลอกไปยังคลิปบอร์ดระบบ"
    }

    fn help_desc_paste_system(&self) -> &str {
        "วางจากคลิปบอร์ดระบบ"
    }

    fn help_desc_cut_system(&self) -> &str {
        "ตัดไปยังคลิปบอร์ดระบบ"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "วางจากคลิปบอร์ดระบบ"
    }

    fn help_desc_undo(&self) -> &str {
        "ยกเลิก"
    }

    fn help_desc_redo(&self) -> &str {
        "ทำซ้ำ"
    }

    fn help_desc_git_ignored(&self) -> &str {
        "สีเทาเข้ม - ถูกเพิกเฉยโดย git"
    }

    fn help_desc_git_modified(&self) -> &str {
        "สีเหลือง - ถูกแก้ไข"
    }

    fn help_desc_git_added(&self) -> &str {
        "สีเขียว - ใหม่/เพิ่มแล้ว"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "สีแดงจาง - ถูกลบ (อ่านอย่างเดียว)"
    }

    fn status_file_created(&self, name: &str) -> String {
        format!("สร้างไฟล์ '{}' แล้ว", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("ข้อผิดพลาดในการสร้างไฟล์: {}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("สร้างไดเรกทอรี '{}' แล้ว", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("ข้อผิดพลาดในการสร้างไดเรกทอรี: {}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "ลบรายการแล้ว"
    }

    fn status_error_delete(&self) -> &str {
        "ข้อผิดพลาดในการลบ"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("ลบ {} รายการแล้ว", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("ลบแล้ว: {}, ข้อผิดพลาด: {}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("บันทึกไฟล์ '{}' แล้ว", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("ข้อผิดพลาดในการบันทึก: {}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("ข้อผิดพลาดในการเปิด '{}': {}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("ข้อผิดพลาด {}: {}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("ข้ามการดำเนินการ '{}' แล้ว", name)
    }

    fn action_copied(&self) -> &str {
        "คัดลอกแล้ว"
    }

    fn action_moved(&self) -> &str {
        "ย้ายแล้ว"
    }

    fn action_copying(&self) -> &str {
        "กำลังคัดลอก"
    }

    fn action_moving(&self) -> &str {
        "กำลังย้าย"
    }

    fn modal_copy_single_title(&self, name: &str) -> String {
        format!("คัดลอก '{}'", name)
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        format!("คัดลอก {} องค์ประกอบ", count)
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        format!("ย้าย '{}'", name)
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        format!("ย้าย {} องค์ประกอบ", count)
    }

    fn modal_create_file_title(&self) -> &str {
        "สร้างไฟล์"
    }

    fn modal_create_dir_title(&self) -> &str {
        "สร้างไดเรกทอรี"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("ลบ '{}'", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("ลบ {} องค์ประกอบ", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "บันทึกเป็น"
    }

    fn modal_enter_filename(&self) -> &str {
        "ป้อนชื่อไฟล์:"
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

    fn batch_result_file_copied(&self) -> &str {
        "คัดลอกแล้ว"
    }

    fn batch_result_file_moved(&self) -> &str {
        "ย้ายแล้ว"
    }

    fn batch_result_error_copy(&self) -> &str {
        "ข้อผิดพลาดในการคัดลอก"
    }

    fn batch_result_error_move(&self) -> &str {
        "ข้อผิดพลาดในการย้าย"
    }

    fn batch_result_copied(&self) -> &str {
        "คัดลอกแล้ว"
    }

    fn batch_result_moved(&self) -> &str {
        "ย้ายแล้ว"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("ข้าม: {}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("ข้อผิดพลาด: {}", count)
    }

    fn menu_files(&self) -> &str {
        "ไฟล์"
    }

    fn menu_terminal(&self) -> &str {
        "เทอร์มินัล"
    }

    fn menu_editor(&self) -> &str {
        "ตัวแก้ไข"
    }

    fn menu_debug(&self) -> &str {
        "บันทึก"
    }

    fn menu_preferences(&self) -> &str {
        "ค่ากำหนด"
    }

    fn menu_help(&self) -> &str {
        "ช่วยเหลือ"
    }

    fn menu_quit(&self) -> &str {
        "ออก"
    }

    fn menu_navigate_hint(&self) -> &str {
        "←→ นำทาง | Enter เลือก | Esc ปิด"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M เมนู"
    }

    fn status_dir(&self) -> &str {
        "ไดเรกทอรี:"
    }

    fn status_file(&self) -> &str {
        "ไฟล์:"
    }

    fn status_mod(&self) -> &str {
        "แก้ไข:"
    }

    fn status_owner(&self) -> &str {
        "เจ้าของ:"
    }

    fn status_selected(&self) -> &str {
        "เลือกแล้ว:"
    }

    fn status_pos(&self) -> &str {
        "ตำแหน่ง:"
    }

    fn status_tab(&self) -> &str {
        "แท็บ:"
    }

    fn status_plain_text(&self) -> &str {
        "ข้อความธรรมดา"
    }

    fn status_readonly(&self) -> &str {
        "[RO]"
    }

    fn status_cwd(&self) -> &str {
        "CWD:"
    }

    fn status_shell(&self) -> &str {
        "เชลล์:"
    }

    fn status_terminal(&self) -> &str {
        "เทอร์มินัล:"
    }

    fn status_layout(&self) -> &str {
        "เลย์เอาต์:"
    }

    fn status_panel(&self) -> &str {
        "แผง:"
    }

    fn ui_yes(&self) -> &str {
        "ใช่"
    }

    fn ui_no(&self) -> &str {
        "ไม่"
    }

    fn ui_ok(&self) -> &str {
        "ตกลง"
    }

    fn ui_cancel(&self) -> &str {
        "ยกเลิก"
    }

    fn ui_continue(&self) -> &str {
        "ดำเนินการต่อ"
    }

    fn ui_close(&self) -> &str {
        "ปิด"
    }

    fn ui_enter_confirm(&self) -> &str {
        "Enter - ยืนยัน"
    }

    fn ui_esc_cancel(&self) -> &str {
        "Esc - ยกเลิก"
    }

    fn ui_hint_separator(&self) -> &str {
        " | "
    }

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

    fn file_info_title(&self) -> &str {
        "ข้อมูลไฟล์"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("ข้อมูลไฟล์ '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("ข้อมูลไดเรกทอรี '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("ข้อมูลลิงก์สัญลักษณ์ '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "ชื่อ"
    }

    fn file_info_path(&self) -> &str {
        "เส้นทาง"
    }

    fn file_info_type(&self) -> &str {
        "ประเภท"
    }

    fn file_info_size(&self) -> &str {
        "ขนาด"
    }

    fn file_info_owner(&self) -> &str {
        "เจ้าของ"
    }

    fn file_info_group(&self) -> &str {
        "กลุ่ม"
    }

    fn file_info_created(&self) -> &str {
        "สร้างเมื่อ"
    }

    fn file_info_modified(&self) -> &str {
        "แก้ไขเมื่อ"
    }

    fn file_info_calculating(&self) -> &str {
        "กำลังคำนวณ"
    }

    fn file_info_press_key(&self) -> &str {
        "กดปุ่มใดก็ได้เพื่อปิด"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        if count == 0 {
            "ไม่มีการเปลี่ยนแปลงที่จะคอมมิต".to_string()
        } else {
            format!("{} การเปลี่ยนแปลงที่จะคอมมิต", count)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        if count == 0 {
            "ไม่มีคอมมิตที่จะพุช".to_string()
        } else {
            format!("{} คอมมิตที่จะพุช", count)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        if count == 0 {
            "ไม่มีคอมมิตที่จะดึง".to_string()
        } else {
            format!("{} คอมมิตที่จะดึง", count)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "ไม่อยู่ในดัชนี git"
    }

    fn file_type_directory(&self) -> &str {
        "ไดเรกทอรี"
    }

    fn file_type_file(&self) -> &str {
        "ไฟล์"
    }

    fn file_type_symlink(&self) -> &str {
        "ลิงก์สัญลักษณ์"
    }
}
