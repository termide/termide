use super::Translation;

/// Russian translation
pub struct Russian;

impl Translation for Russian {
    // File Manager operations
    fn fm_copy_files(&self) -> &str {
        "Файлы скопированы в буфер обмена"
    }

    fn fm_cut_files(&self) -> &str {
        "Файлы вырезаны в буфер обмена"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        let mode_ru = match mode {
            "Copy" => "Копировать",
            "Move" => "Переместить",
            _ => mode,
        };
        format!(
            "{} {} {} в:\n{}",
            mode_ru,
            count,
            match count {
                1 => "файл",
                2..=4 => "файла",
                _ => "файлов",
            },
            dest
        )
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!(
            "Удалить {} {}?",
            count,
            match count {
                1 => "файл",
                2..=4 => "файла",
                _ => "файлов",
            }
        )
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("Переименовать '{}' в:", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "Введите имя файла:"
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "Введите имя каталога:"
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("Копировать '{}' в:", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("Переместить '{}' в:", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "Поиск:"
    }

    fn fm_no_results(&self) -> &str {
        "Совпадений не найдено"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "Операция отменена"
    }

    // Modal buttons
    fn modal_yes(&self) -> &str {
        "Да"
    }

    fn modal_no(&self) -> &str {
        "Нет"
    }

    fn modal_ok(&self) -> &str {
        "OK"
    }

    fn modal_cancel(&self) -> &str {
        "Отмена"
    }

    // Panel titles
    fn panel_file_manager(&self) -> &str {
        "Файловый менеджер"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("Редактор: {}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "Терминал"
    }

    fn panel_welcome(&self) -> &str {
        "Добро пожаловать"
    }

    // Editor
    fn editor_close_unsaved(&self) -> &str {
        "Закрыть редактор"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "Файл содержит несохраненные изменения. Что делать?"
    }

    fn editor_save_and_close(&self) -> &str {
        "Сохранить и закрыть"
    }

    fn editor_close_without_saving(&self) -> &str {
        "Закрыть без сохранения"
    }

    fn editor_cancel(&self) -> &str {
        "Отмена"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("Не удалось сохранить файл: {}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("Файл сохранен: {}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("Файл '{}' открыт", filename)
    }

    fn editor_search_title(&self) -> &str {
        "Поиск"
    }

    fn editor_search_prompt(&self) -> &str {
        "Введите строку для поиска:"
    }

    fn editor_replace_title(&self) -> &str {
        "Замена"
    }

    fn editor_replace_prompt(&self) -> &str {
        "Найти:"
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "Заменить на:"
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("Совпадение {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "Нет совпадений"
    }

    // Terminal
    fn terminal_exit_confirm(&self) -> &str {
        "Процесс еще выполняется. Закрыть терминал?"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("Процесс завершен с кодом {}", code)
    }

    // Git status
    fn git_detected(&self) -> &str {
        "Git обнаружен и доступен"
    }

    fn git_not_found(&self) -> &str {
        "Git не найден - интеграция с git отключена"
    }

    fn app_quit_confirm(&self) -> &str {
        "Есть несохранённые изменения. Всё равно выйти?"
    }

    // Errors
    fn error_operation_failed(&self, error: &str) -> String {
        format!("Операция не выполнена: {}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("Файл или каталог уже существует: {}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "Неверный путь"
    }

    fn error_source_eq_dest(&self) -> &str {
        "Источник и назначение совпадают"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "Назначение является подкаталогом источника"
    }

    // Help modal
    fn help_title(&self) -> &str {
        "Справка"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - Справка"
    }

    fn help_version(&self) -> &str {
        "0.1.4"
    }

    fn help_global_keys(&self) -> &str {
        "ОСНОВНЫЕ ГОРЯЧИЕ КЛАВИШИ"
    }

    fn help_file_manager_keys(&self) -> &str {
        "ФАЙЛОВЫЙ МЕНЕДЖЕР"
    }

    fn help_editor_keys(&self) -> &str {
        "ТЕКСТОВЫЙ РЕДАКТОР"
    }

    fn help_terminal_keys(&self) -> &str {
        "ТЕРМИНАЛ"
    }

    fn help_git_integration(&self) -> &str {
        "ИНТЕГРАЦИЯ С GIT"
    }

    fn help_clipboard_operations(&self) -> &str {
        "ОПЕРАЦИИ С БУФЕРОМ ОБМЕНА"
    }

    fn help_close_hint(&self) -> &str {
        "Нажмите Esc или Ctrl+H чтобы закрыть это окно"
    }

    // Help key descriptions - Global
    fn help_desc_menu(&self) -> &str {
        "Открыть/закрыть меню"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "Меню панелей"
    }

    fn help_desc_quit(&self) -> &str {
        "Выход из приложения"
    }

    fn help_desc_help(&self) -> &str {
        "Показать справку"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "Переключение между панелями"
    }

    fn help_desc_close_panel(&self) -> &str {
        "Закрыть активную панель"
    }

    // Help key descriptions - File Manager
    fn help_desc_navigate(&self) -> &str {
        "Навигация по списку"
    }

    fn help_desc_select(&self) -> &str {
        "Переключить выбор"
    }

    fn help_desc_select_all(&self) -> &str {
        "Выбрать все файлы"
    }

    fn help_desc_open_file(&self) -> &str {
        "Открыть директорию или файл в редакторе"
    }

    fn help_desc_open_editor(&self) -> &str {
        "Открыть файл в редакторе"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "Открыть новый терминал в текущем каталоге"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "Перейти в родительскую директорию"
    }

    fn help_desc_home(&self) -> &str {
        "Перейти в начало списка"
    }

    fn help_desc_end(&self) -> &str {
        "Перейти в конец списка"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "Прокрутка страницами"
    }

    fn help_desc_create_file(&self) -> &str {
        "Создать новый файл"
    }

    fn help_desc_create_dir(&self) -> &str {
        "Создать новый каталог"
    }

    fn help_desc_rename(&self) -> &str {
        "Переименовать файл/каталог"
    }

    fn help_desc_copy(&self) -> &str {
        "Копировать файл/каталог"
    }

    fn help_desc_move(&self) -> &str {
        "Переместить файл/каталог"
    }

    fn help_desc_delete(&self) -> &str {
        "Удалить выбранный файл/каталог"
    }

    fn help_desc_search(&self) -> &str {
        "Поиск файлов"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "Копировать выбранные файлы в файловый буфер"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "Вырезать выбранные файлы в файловый буфер"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "Вставить файлы из файлового буфера"
    }

    // Help key descriptions - Editor
    fn help_desc_save(&self) -> &str {
        "Сохранить файл"
    }

    fn help_desc_copy_system(&self) -> &str {
        "Копировать в системный буфер обмена"
    }

    fn help_desc_paste_system(&self) -> &str {
        "Вставить из системного буфера обмена"
    }

    fn help_desc_cut_system(&self) -> &str {
        "Вырезать в системный буфер обмена"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "Вставить из системного буфера обмена"
    }

    fn help_desc_undo(&self) -> &str {
        "Отменить"
    }

    fn help_desc_redo(&self) -> &str {
        "Повторить"
    }

    // Help key descriptions - Git colors
    fn help_desc_git_ignored(&self) -> &str {
        "Темно-серый - игнорируется git"
    }

    fn help_desc_git_modified(&self) -> &str {
        "Желтый - изменено"
    }

    fn help_desc_git_added(&self) -> &str {
        "Зеленый - новое/добавлено"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "Блеклый красный - удалено (только чтение)"
    }

    // File operation status messages
    fn status_file_created(&self, name: &str) -> String {
        format!("Файл '{}' создан", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("Ошибка создания файла: {}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("Каталог '{}' создан", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("Ошибка создания каталога: {}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "Элемент удалён"
    }

    fn status_error_delete(&self) -> &str {
        "Ошибка удаления"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("Удалено {} элементов", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("Удалено: {}, ошибок: {}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("Файл '{}' сохранён", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("Ошибка сохранения: {}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("Ошибка открытия '{}': {}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("Ошибка {}: {}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("Операция '{}' пропущена", name)
    }

    // Action words
    fn action_copied(&self) -> &str {
        "скопирован"
    }

    fn action_moved(&self) -> &str {
        "перемещён"
    }

    fn action_copying(&self) -> &str {
        "копирования"
    }

    fn action_moving(&self) -> &str {
        "перемещения"
    }

    // Modal titles and prompts for copy/move
    fn modal_copy_single_title(&self, name: &str) -> String {
        format!("Копировать '{}'", name)
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        format!("Копировать {} элементов", count)
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        format!("Переместить '{}'", name)
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        format!("Переместить {} элементов", count)
    }

    fn modal_create_file_title(&self) -> &str {
        "Создать файл"
    }

    fn modal_create_dir_title(&self) -> &str {
        "Создать каталог"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("Удалить '{}'", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("Удалить {} элементов", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "Сохранить как"
    }

    fn modal_enter_filename(&self) -> &str {
        "Введите имя файла:"
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
        "скопировано"
    }

    fn batch_result_file_moved(&self) -> &str {
        "перемещено"
    }

    fn batch_result_error_copy(&self) -> &str {
        "Ошибка копирования"
    }

    fn batch_result_error_move(&self) -> &str {
        "Ошибка перемещения"
    }

    fn batch_result_copied(&self) -> &str {
        "Скопировано"
    }

    fn batch_result_moved(&self) -> &str {
        "Перемещено"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("пропущено: {}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("ошибок: {}", count)
    }

    // Menu items
    fn menu_files(&self) -> &str {
        "Файлы"
    }

    fn menu_terminal(&self) -> &str {
        "Терминал"
    }

    fn menu_editor(&self) -> &str {
        "Редактор"
    }

    fn menu_debug(&self) -> &str {
        "Отладка"
    }

    fn menu_preferences(&self) -> &str {
        "Настройки"
    }

    fn menu_help(&self) -> &str {
        "Помощь"
    }

    fn menu_quit(&self) -> &str {
        "Выход"
    }

    // Menu hints
    fn menu_navigate_hint(&self) -> &str {
        "←→ Навигация | Enter Выбор | Esc Закрыть"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M Меню"
    }

    // Status bar labels
    fn status_dir(&self) -> &str {
        "Каталог:"
    }

    fn status_file(&self) -> &str {
        "Файл:"
    }

    fn status_mod(&self) -> &str {
        "Права:"
    }

    fn status_owner(&self) -> &str {
        "Владелец:"
    }

    fn status_selected(&self) -> &str {
        "Выбрано:"
    }

    fn status_pos(&self) -> &str {
        "Позиция:"
    }

    fn status_tab(&self) -> &str {
        "Табуляция:"
    }

    fn status_plain_text(&self) -> &str {
        "Обычный текст"
    }

    fn status_readonly(&self) -> &str {
        "[Только чтение]"
    }

    fn status_cwd(&self) -> &str {
        "Рабочий каталог:"
    }

    fn status_shell(&self) -> &str {
        "Оболочка:"
    }

    fn status_terminal(&self) -> &str {
        "Терминал:"
    }

    fn status_layout(&self) -> &str {
        "Разметка:"
    }

    fn status_panel(&self) -> &str {
        "Панель:"
    }

    // UI elements
    fn ui_yes(&self) -> &str {
        "Да"
    }

    fn ui_no(&self) -> &str {
        "Нет"
    }

    fn ui_ok(&self) -> &str {
        "ОК"
    }

    fn ui_cancel(&self) -> &str {
        "Отмена"
    }

    fn ui_continue(&self) -> &str {
        "Продолжить"
    }

    fn ui_close(&self) -> &str {
        "Закрыть"
    }

    fn ui_enter_confirm(&self) -> &str {
        "Enter - подтвердить"
    }

    fn ui_esc_cancel(&self) -> &str {
        "Esc - отменить"
    }

    fn ui_hint_separator(&self) -> &str {
        " | "
    }

    // File size units
    fn size_bytes(&self) -> &str {
        "Б"
    }

    fn size_kilobytes(&self) -> &str {
        "КБ"
    }

    fn size_megabytes(&self) -> &str {
        "МБ"
    }

    fn size_gigabytes(&self) -> &str {
        "ГБ"
    }

    fn size_terabytes(&self) -> &str {
        "ТБ"
    }

    // File info modal
    fn file_info_title(&self) -> &str {
        "Свойства файла"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("Свойства файла '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("Свойства каталога '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("Свойства ссылки '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "Имя"
    }

    fn file_info_path(&self) -> &str {
        "Путь"
    }

    fn file_info_type(&self) -> &str {
        "Тип"
    }

    fn file_info_size(&self) -> &str {
        "Размер"
    }

    fn file_info_owner(&self) -> &str {
        "Владелец"
    }

    fn file_info_group(&self) -> &str {
        "Группа"
    }

    fn file_info_created(&self) -> &str {
        "Создан"
    }

    fn file_info_modified(&self) -> &str {
        "Изменён"
    }

    fn file_info_calculating(&self) -> &str {
        "Вычисляется"
    }

    fn file_info_press_key(&self) -> &str {
        "Нажмите любую клавишу для закрытия"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        // Helper function for Russian pluralization
        fn pluralize(n: usize, one: &str, few: &str, many: &str) -> String {
            let last_digit = n % 10;
            let last_two_digits = n % 100;

            if (11..=19).contains(&last_two_digits) {
                format!("{} {}", n, many)
            } else if last_digit == 1 {
                format!("{} {}", n, one)
            } else if (2..=4).contains(&last_digit) {
                format!("{} {}", n, few)
            } else {
                format!("{} {}", n, many)
            }
        }

        if count == 0 {
            "нет изменений для коммита".to_string()
        } else {
            let changes = pluralize(count, "изменение", "изменения", "изменений");
            format!("{} для коммита", changes)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        fn pluralize(n: usize, one: &str, few: &str, many: &str) -> String {
            let last_digit = n % 10;
            let last_two_digits = n % 100;

            if (11..=19).contains(&last_two_digits) {
                format!("{} {}", n, many)
            } else if last_digit == 1 {
                format!("{} {}", n, one)
            } else if (2..=4).contains(&last_digit) {
                format!("{} {}", n, few)
            } else {
                format!("{} {}", n, many)
            }
        }

        if count == 0 {
            "нет коммитов для отправки".to_string()
        } else {
            let commits = pluralize(count, "коммит", "коммита", "коммитов");
            format!("{} для отправки", commits)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        fn pluralize(n: usize, one: &str, few: &str, many: &str) -> String {
            let last_digit = n % 10;
            let last_two_digits = n % 100;

            if (11..=19).contains(&last_two_digits) {
                format!("{} {}", n, many)
            } else if last_digit == 1 {
                format!("{} {}", n, one)
            } else if (2..=4).contains(&last_digit) {
                format!("{} {}", n, few)
            } else {
                format!("{} {}", n, many)
            }
        }

        if count == 0 {
            "нет коммитов для получения".to_string()
        } else {
            let commits = pluralize(count, "коммит", "коммита", "коммитов");
            format!("{} для получения", commits)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "не в индексе git"
    }

    // File types
    fn file_type_directory(&self) -> &str {
        "Каталог"
    }

    fn file_type_file(&self) -> &str {
        "Файл"
    }

    fn file_type_symlink(&self) -> &str {
        "Символьная ссылка"
    }
}
