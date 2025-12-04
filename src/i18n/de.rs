use super::Translation;

/// German translation
/// Deutsche Übersetzung
pub struct German;

impl Translation for German {
    // File Manager operations
    fn fm_copy_files(&self) -> &str {
        "Dateien in Zwischenablage kopiert"
    }

    fn fm_cut_files(&self) -> &str {
        "Dateien in Zwischenablage ausgeschnitten"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        format!(
            "{} {} Datei{} nach:\n{}",
            mode,
            count,
            if count == 1 { "" } else { "en" },
            dest
        )
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!(
            "{} Datei{} löschen?",
            count,
            if count == 1 { "" } else { "en" }
        )
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("'{}' umbenennen in:", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "Dateiname eingeben:"
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "Verzeichnisname eingeben:"
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("'{}' kopieren nach:", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("'{}' verschieben nach:", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "Suchen:"
    }

    fn fm_no_results(&self) -> &str {
        "Keine passenden Dateien gefunden"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "Operation abgebrochen"
    }

    // Modal buttons
    fn modal_yes(&self) -> &str {
        "Ja"
    }

    fn modal_no(&self) -> &str {
        "Nein"
    }

    fn modal_ok(&self) -> &str {
        "OK"
    }

    fn modal_cancel(&self) -> &str {
        "Abbrechen"
    }

    // Panel titles
    fn panel_file_manager(&self) -> &str {
        "Dateimanager"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("Editor: {}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "Terminal"
    }

    fn panel_welcome(&self) -> &str {
        "Willkommen"
    }

    // Editor
    fn editor_close_unsaved(&self) -> &str {
        "Editor schließen"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "Datei hat ungespeicherte Änderungen. Was tun?"
    }

    fn editor_save_and_close(&self) -> &str {
        "Speichern und schließen"
    }

    fn editor_close_without_saving(&self) -> &str {
        "Ohne Speichern schließen"
    }

    fn editor_cancel(&self) -> &str {
        "Abbrechen"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("Fehler beim Speichern: {}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("Datei gespeichert: {}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("Datei '{}' geöffnet", filename)
    }

    fn editor_search_title(&self) -> &str {
        "Suchen"
    }

    fn editor_search_prompt(&self) -> &str {
        "Suchbegriff eingeben:"
    }

    fn editor_replace_title(&self) -> &str {
        "Ersetzen"
    }

    fn editor_replace_prompt(&self) -> &str {
        "Suchen nach:"
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "Ersetzen durch:"
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("Treffer {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "Keine Treffer"
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        format!(
            "{} Zeile{} gelöscht",
            count,
            if count == 1 { "" } else { "n" }
        )
    }

    // Terminal
    fn terminal_exit_confirm(&self) -> &str {
        "Prozess läuft noch. Terminal schließen?"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("Prozess beendet mit Code {}", code)
    }

    // Git status
    fn git_detected(&self) -> &str {
        "Git erkannt und verfügbar"
    }

    fn git_not_found(&self) -> &str {
        "Git nicht gefunden - Git-Integration deaktiviert"
    }

    fn app_quit_confirm(&self) -> &str {
        "Es gibt ungespeicherte Änderungen. Trotzdem beenden?"
    }

    // Errors
    fn error_operation_failed(&self, error: &str) -> String {
        format!("Operation fehlgeschlagen: {}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("Datei oder Verzeichnis existiert bereits: {}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "Ungültiger Pfad"
    }

    fn error_source_eq_dest(&self) -> &str {
        "Quelle und Ziel sind identisch"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "Ziel ist Unterverzeichnis der Quelle"
    }

    // Help modal
    fn help_title(&self) -> &str {
        "Hilfe"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - Hilfe"
    }

    fn help_version(&self) -> &str {
        "0.3.0"
    }

    fn help_global_keys(&self) -> &str {
        "GLOBALE HOTKEYS"
    }

    fn help_file_manager_keys(&self) -> &str {
        "DATEIMANAGER"
    }

    fn help_editor_keys(&self) -> &str {
        "TEXTEDITOR"
    }

    fn help_terminal_keys(&self) -> &str {
        "TERMINAL"
    }

    fn help_git_integration(&self) -> &str {
        "GIT-INTEGRATION"
    }

    fn help_clipboard_operations(&self) -> &str {
        "ZWISCHENABLAGE"
    }

    fn help_close_hint(&self) -> &str {
        "Drücken Sie Esc oder Ctrl+H zum Schließen"
    }

    // Help key descriptions - Global
    fn help_desc_menu(&self) -> &str {
        "Menü öffnen/schließen"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "Panel-Menü"
    }

    fn help_desc_quit(&self) -> &str {
        "Anwendung beenden"
    }

    fn help_desc_help(&self) -> &str {
        "Hilfe anzeigen"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "Zwischen Panels wechseln"
    }

    fn help_desc_close_panel(&self) -> &str {
        "Aktives Panel schließen"
    }

    // Help key descriptions - File Manager
    fn help_desc_navigate(&self) -> &str {
        "Durch Liste navigieren"
    }

    fn help_desc_select(&self) -> &str {
        "Auswahl umschalten"
    }

    fn help_desc_select_all(&self) -> &str {
        "Alle Dateien auswählen"
    }

    fn help_desc_open_file(&self) -> &str {
        "Verzeichnis oder Datei im Editor öffnen"
    }

    fn help_desc_open_editor(&self) -> &str {
        "Datei im Editor öffnen"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "Neues Terminal im aktuellen Verzeichnis öffnen"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "Zum übergeordneten Verzeichnis wechseln"
    }

    fn help_desc_home(&self) -> &str {
        "Zum Listenanfang springen"
    }

    fn help_desc_end(&self) -> &str {
        "Zum Listenende springen"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "Seitenweise scrollen"
    }

    fn help_desc_create_file(&self) -> &str {
        "Neue Datei erstellen"
    }

    fn help_desc_create_dir(&self) -> &str {
        "Neues Verzeichnis erstellen"
    }

    fn help_desc_rename(&self) -> &str {
        "Datei/Verzeichnis umbenennen"
    }

    fn help_desc_copy(&self) -> &str {
        "Datei/Verzeichnis kopieren"
    }

    fn help_desc_move(&self) -> &str {
        "Datei/Verzeichnis verschieben"
    }

    fn help_desc_delete(&self) -> &str {
        "Ausgewählte Datei/Verzeichnis löschen"
    }

    fn help_desc_search(&self) -> &str {
        "Dateien suchen"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "Ausgewählte Dateien in Zwischenablage kopieren"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "Ausgewählte Dateien in Zwischenablage ausschneiden"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "Dateien aus Zwischenablage einfügen"
    }

    // Help key descriptions - Editor
    fn help_desc_save(&self) -> &str {
        "Datei speichern"
    }

    fn help_desc_copy_system(&self) -> &str {
        "In Systemzwischenablage kopieren"
    }

    fn help_desc_paste_system(&self) -> &str {
        "Aus Systemzwischenablage einfügen"
    }

    fn help_desc_cut_system(&self) -> &str {
        "In Systemzwischenablage ausschneiden"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "Aus Systemzwischenablage einfügen"
    }

    fn help_desc_undo(&self) -> &str {
        "Rückgängig"
    }

    fn help_desc_redo(&self) -> &str {
        "Wiederholen"
    }

    // Help key descriptions - Git colors
    fn help_desc_git_ignored(&self) -> &str {
        "Dunkelgrau - von Git ignoriert"
    }

    fn help_desc_git_modified(&self) -> &str {
        "Gelb - geändert"
    }

    fn help_desc_git_added(&self) -> &str {
        "Grün - neu/hinzugefügt"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "Blassrot - gelöscht (schreibgeschützt)"
    }

    // File operation status messages
    fn status_file_created(&self, name: &str) -> String {
        format!("Datei '{}' erstellt", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("Fehler beim Erstellen der Datei: {}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("Verzeichnis '{}' erstellt", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("Fehler beim Erstellen des Verzeichnisses: {}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "Element gelöscht"
    }

    fn status_error_delete(&self) -> &str {
        "Fehler beim Löschen"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("{} Elemente gelöscht", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("Gelöscht: {}, Fehler: {}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("Datei '{}' gespeichert", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("Fehler beim Speichern: {}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("Fehler beim Öffnen von '{}': {}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("Fehler {}: {}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("Operation '{}' übersprungen", name)
    }

    // Action words
    fn action_copied(&self) -> &str {
        "kopiert"
    }

    fn action_moved(&self) -> &str {
        "verschoben"
    }

    fn action_copying(&self) -> &str {
        "kopieren"
    }

    fn action_moving(&self) -> &str {
        "verschieben"
    }

    // Modal titles and prompts for copy/move
    fn modal_copy_single_title(&self, name: &str) -> String {
        format!("'{}' kopieren", name)
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        format!("{} Elemente kopieren", count)
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        format!("'{}' verschieben", name)
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        format!("{} Elemente verschieben", count)
    }

    fn modal_create_file_title(&self) -> &str {
        "Datei erstellen"
    }

    fn modal_create_dir_title(&self) -> &str {
        "Verzeichnis erstellen"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("'{}' löschen", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("{} Elemente löschen", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "Speichern unter"
    }

    fn modal_enter_filename(&self) -> &str {
        "Dateiname eingeben:"
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
        "kopiert"
    }

    fn batch_result_file_moved(&self) -> &str {
        "verschoben"
    }

    fn batch_result_error_copy(&self) -> &str {
        "Fehler beim Kopieren"
    }

    fn batch_result_error_move(&self) -> &str {
        "Fehler beim Verschieben"
    }

    fn batch_result_copied(&self) -> &str {
        "Kopiert"
    }

    fn batch_result_moved(&self) -> &str {
        "Verschoben"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("übersprungen: {}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("Fehler: {}", count)
    }

    // Menu items
    fn menu_files(&self) -> &str {
        "Dateien"
    }

    fn menu_terminal(&self) -> &str {
        "Terminal"
    }

    fn menu_editor(&self) -> &str {
        "Editor"
    }

    fn menu_debug(&self) -> &str {
        "Log"
    }

    fn menu_preferences(&self) -> &str {
        "Einstellungen"
    }

    fn menu_help(&self) -> &str {
        "Hilfe"
    }

    fn menu_quit(&self) -> &str {
        "Beenden"
    }

    // Menu hints
    fn menu_navigate_hint(&self) -> &str {
        "←→ Navigieren | Enter Auswählen | Esc Schließen"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M Menü"
    }

    // Status bar labels
    fn status_dir(&self) -> &str {
        "Verz:"
    }

    fn status_file(&self) -> &str {
        "Datei:"
    }

    fn status_mod(&self) -> &str {
        "Mod:"
    }

    fn status_owner(&self) -> &str {
        "Besitzer:"
    }

    fn status_selected(&self) -> &str {
        "Ausgewählt:"
    }

    fn status_pos(&self) -> &str {
        "Pos:"
    }

    fn status_tab(&self) -> &str {
        "Tab:"
    }

    fn status_plain_text(&self) -> &str {
        "Klartext"
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
        "Ja"
    }

    fn ui_no(&self) -> &str {
        "Nein"
    }

    fn ui_ok(&self) -> &str {
        "OK"
    }

    fn ui_cancel(&self) -> &str {
        "Abbrechen"
    }

    fn ui_continue(&self) -> &str {
        "Fortfahren"
    }

    fn ui_close(&self) -> &str {
        "Schließen"
    }

    fn ui_enter_confirm(&self) -> &str {
        "Enter - bestätigen"
    }

    fn ui_esc_cancel(&self) -> &str {
        "Esc - abbrechen"
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
        "Datei-Informationen"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("Datei-Info '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("Verzeichnis-Info '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("Symlink-Info '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "Name"
    }

    fn file_info_path(&self) -> &str {
        "Pfad"
    }

    fn file_info_type(&self) -> &str {
        "Typ"
    }

    fn file_info_size(&self) -> &str {
        "Größe"
    }

    fn file_info_owner(&self) -> &str {
        "Besitzer"
    }

    fn file_info_group(&self) -> &str {
        "Gruppe"
    }

    fn file_info_created(&self) -> &str {
        "Erstellt"
    }

    fn file_info_modified(&self) -> &str {
        "Geändert"
    }

    fn file_info_calculating(&self) -> &str {
        "Berechne"
    }

    fn file_info_press_key(&self) -> &str {
        "Beliebige Taste zum Schließen drücken"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        if count == 0 {
            "keine zu committenden Änderungen".to_string()
        } else if count == 1 {
            "1 zu committende Änderung".to_string()
        } else {
            format!("{} zu committende Änderungen", count)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        if count == 0 {
            "keine zu pushenden Commits".to_string()
        } else if count == 1 {
            "1 zu pushender Commit".to_string()
        } else {
            format!("{} zu pushende Commits", count)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        if count == 0 {
            "keine zu pullenden Commits".to_string()
        } else if count == 1 {
            "1 zu pullender Commit".to_string()
        } else {
            format!("{} zu pullende Commits", count)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "nicht im Git-Index"
    }

    // File types
    fn file_type_directory(&self) -> &str {
        "Verzeichnis"
    }

    fn file_type_file(&self) -> &str {
        "Datei"
    }

    fn file_type_symlink(&self) -> &str {
        "Symlink"
    }
}
