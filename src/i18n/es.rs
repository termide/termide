use super::Translation;

/// Spanish translation
/// Traducción en español
pub struct Spanish;

impl Translation for Spanish {
    // File Manager operations
    fn fm_copy_files(&self) -> &str {
        "Archivos copiados al portapapeles"
    }

    fn fm_cut_files(&self) -> &str {
        "Archivos cortados al portapapeles"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        format!(
            "{} {} archivo{} a:\n{}",
            mode,
            count,
            if count == 1 { "" } else { "s" },
            dest
        )
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!(
            "¿Eliminar {} archivo{}?",
            count,
            if count == 1 { "" } else { "s" }
        )
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("Renombrar '{}' a:", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "Ingrese el nombre del archivo:"
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "Ingrese el nombre del directorio:"
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("Copiar '{}' a:", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("Mover '{}' a:", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "Buscar:"
    }

    fn fm_no_results(&self) -> &str {
        "No se encontraron archivos coincidentes"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "Operación cancelada"
    }

    // Modal buttons
    fn modal_yes(&self) -> &str {
        "Sí"
    }

    fn modal_no(&self) -> &str {
        "No"
    }

    fn modal_ok(&self) -> &str {
        "OK"
    }

    fn modal_cancel(&self) -> &str {
        "Cancelar"
    }

    // Panel titles
    fn panel_file_manager(&self) -> &str {
        "Gestor de Archivos"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("Editor: {}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "Terminal"
    }

    fn panel_welcome(&self) -> &str {
        "Bienvenido"
    }

    // Editor
    fn editor_close_unsaved(&self) -> &str {
        "Cerrar Editor"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "El archivo tiene cambios no guardados. ¿Qué hacer?"
    }

    fn editor_save_and_close(&self) -> &str {
        "Guardar y cerrar"
    }

    fn editor_close_without_saving(&self) -> &str {
        "Cerrar sin guardar"
    }

    fn editor_cancel(&self) -> &str {
        "Cancelar"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("Error al guardar el archivo: {}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("Archivo guardado: {}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("Archivo '{}' abierto", filename)
    }

    fn editor_search_title(&self) -> &str {
        "Buscar"
    }

    fn editor_search_prompt(&self) -> &str {
        "Ingrese la búsqueda:"
    }

    fn editor_replace_title(&self) -> &str {
        "Reemplazar"
    }

    fn editor_replace_prompt(&self) -> &str {
        "Buscar:"
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "Reemplazar con:"
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("Coincidencia {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "Sin coincidencias"
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        format!(
            "{} línea{} eliminada{}",
            count,
            if count == 1 { "" } else { "s" },
            if count == 1 { "" } else { "s" }
        )
    }

    // Terminal
    fn terminal_exit_confirm(&self) -> &str {
        "El proceso aún está en ejecución. ¿Cerrar terminal?"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("Proceso terminado con código {}", code)
    }

    // Git status
    fn git_detected(&self) -> &str {
        "Git detectado y disponible"
    }

    fn git_not_found(&self) -> &str {
        "Git no encontrado - integración git deshabilitada"
    }

    fn app_quit_confirm(&self) -> &str {
        "Hay cambios no guardados. ¿Salir de todos modos?"
    }

    // Errors
    fn error_operation_failed(&self, error: &str) -> String {
        format!("Operación fallida: {}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("El archivo o directorio ya existe: {}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "Ruta inválida"
    }

    fn error_source_eq_dest(&self) -> &str {
        "Origen y destino son iguales"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "El destino es un subdirectorio del origen"
    }

    // Help modal
    fn help_title(&self) -> &str {
        "Ayuda"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - Ayuda"
    }

    fn help_version(&self) -> &str {
        "0.3.0"
    }

    fn help_global_keys(&self) -> &str {
        "ATAJOS GLOBALES"
    }

    fn help_file_manager_keys(&self) -> &str {
        "GESTOR DE ARCHIVOS"
    }

    fn help_editor_keys(&self) -> &str {
        "EDITOR DE TEXTO"
    }

    fn help_terminal_keys(&self) -> &str {
        "TERMINAL"
    }

    fn help_git_integration(&self) -> &str {
        "INTEGRACIÓN GIT"
    }

    fn help_clipboard_operations(&self) -> &str {
        "OPERACIONES DE PORTAPAPELES"
    }

    fn help_close_hint(&self) -> &str {
        "Presione Esc o Ctrl+H para cerrar esta ventana"
    }

    // Help key descriptions - Global
    fn help_desc_menu(&self) -> &str {
        "Abrir/cerrar menú"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "Menú del panel"
    }

    fn help_desc_quit(&self) -> &str {
        "Salir de la aplicación"
    }

    fn help_desc_help(&self) -> &str {
        "Mostrar ayuda"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "Cambiar entre paneles"
    }

    fn help_desc_close_panel(&self) -> &str {
        "Cerrar panel activo"
    }

    // Help key descriptions - File Manager
    fn help_desc_navigate(&self) -> &str {
        "Navegar por la lista"
    }

    fn help_desc_select(&self) -> &str {
        "Alternar selección"
    }

    fn help_desc_select_all(&self) -> &str {
        "Seleccionar todos los archivos"
    }

    fn help_desc_open_file(&self) -> &str {
        "Abrir directorio o archivo en editor"
    }

    fn help_desc_open_editor(&self) -> &str {
        "Abrir archivo en editor"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "Abrir nueva terminal en el directorio actual"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "Ir al directorio padre"
    }

    fn help_desc_home(&self) -> &str {
        "Ir al inicio de la lista"
    }

    fn help_desc_end(&self) -> &str {
        "Ir al final de la lista"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "Desplazar por páginas"
    }

    fn help_desc_create_file(&self) -> &str {
        "Crear nuevo archivo"
    }

    fn help_desc_create_dir(&self) -> &str {
        "Crear nuevo directorio"
    }

    fn help_desc_rename(&self) -> &str {
        "Renombrar archivo/directorio"
    }

    fn help_desc_copy(&self) -> &str {
        "Copiar archivo/directorio"
    }

    fn help_desc_move(&self) -> &str {
        "Mover archivo/directorio"
    }

    fn help_desc_delete(&self) -> &str {
        "Eliminar archivo/directorio seleccionado"
    }

    fn help_desc_search(&self) -> &str {
        "Buscar archivos"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "Copiar archivos seleccionados al portapapeles"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "Cortar archivos seleccionados al portapapeles"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "Pegar archivos del portapapeles"
    }

    // Help key descriptions - Editor
    fn help_desc_save(&self) -> &str {
        "Guardar archivo"
    }

    fn help_desc_copy_system(&self) -> &str {
        "Copiar al portapapeles del sistema"
    }

    fn help_desc_paste_system(&self) -> &str {
        "Pegar del portapapeles del sistema"
    }

    fn help_desc_cut_system(&self) -> &str {
        "Cortar al portapapeles del sistema"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "Pegar del portapapeles del sistema"
    }

    fn help_desc_undo(&self) -> &str {
        "Deshacer"
    }

    fn help_desc_redo(&self) -> &str {
        "Rehacer"
    }

    // Help key descriptions - Git colors
    fn help_desc_git_ignored(&self) -> &str {
        "Gris oscuro - ignorado por git"
    }

    fn help_desc_git_modified(&self) -> &str {
        "Amarillo - modificado"
    }

    fn help_desc_git_added(&self) -> &str {
        "Verde - nuevo/agregado"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "Rojo pálido - eliminado (solo lectura)"
    }

    // File operation status messages
    fn status_file_created(&self, name: &str) -> String {
        format!("Archivo '{}' creado", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("Error al crear archivo: {}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("Directorio '{}' creado", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("Error al crear directorio: {}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "Elemento eliminado"
    }

    fn status_error_delete(&self) -> &str {
        "Error al eliminar"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("{} elementos eliminados", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("Eliminados: {}, errores: {}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("Archivo '{}' guardado", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("Error al guardar: {}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("Error al abrir '{}': {}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("Error {}: {}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("Operación '{}' omitida", name)
    }

    // Action words
    fn action_copied(&self) -> &str {
        "copiado"
    }

    fn action_moved(&self) -> &str {
        "movido"
    }

    fn action_copying(&self) -> &str {
        "copiando"
    }

    fn action_moving(&self) -> &str {
        "moviendo"
    }

    // Modal titles and prompts for copy/move
    fn modal_copy_single_title(&self, name: &str) -> String {
        format!("Copiar '{}'", name)
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        format!("Copiar {} elementos", count)
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        format!("Mover '{}'", name)
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        format!("Mover {} elementos", count)
    }

    fn modal_create_file_title(&self) -> &str {
        "Crear Archivo"
    }

    fn modal_create_dir_title(&self) -> &str {
        "Crear Directorio"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("Eliminar '{}'", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("Eliminar {} elementos", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "Guardar Como"
    }

    fn modal_enter_filename(&self) -> &str {
        "Ingrese el nombre del archivo:"
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
        "copiado"
    }

    fn batch_result_file_moved(&self) -> &str {
        "movido"
    }

    fn batch_result_error_copy(&self) -> &str {
        "Error al copiar"
    }

    fn batch_result_error_move(&self) -> &str {
        "Error al mover"
    }

    fn batch_result_copied(&self) -> &str {
        "Copiado"
    }

    fn batch_result_moved(&self) -> &str {
        "Movido"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("omitidos: {}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("errores: {}", count)
    }

    // Menu items
    fn menu_files(&self) -> &str {
        "Archivos"
    }

    fn menu_terminal(&self) -> &str {
        "Terminal"
    }

    fn menu_editor(&self) -> &str {
        "Editor"
    }

    fn menu_debug(&self) -> &str {
        "Registro"
    }

    fn menu_preferences(&self) -> &str {
        "Preferencias"
    }

    fn menu_help(&self) -> &str {
        "Ayuda"
    }

    fn menu_quit(&self) -> &str {
        "Salir"
    }

    // Menu hints
    fn menu_navigate_hint(&self) -> &str {
        "←→ Navegar | Enter Seleccionar | Esc Cerrar"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M Menú"
    }

    // Status bar labels
    fn status_dir(&self) -> &str {
        "Dir:"
    }

    fn status_file(&self) -> &str {
        "Archivo:"
    }

    fn status_mod(&self) -> &str {
        "Mod:"
    }

    fn status_owner(&self) -> &str {
        "Propietario:"
    }

    fn status_selected(&self) -> &str {
        "Seleccionados:"
    }

    fn status_pos(&self) -> &str {
        "Pos:"
    }

    fn status_tab(&self) -> &str {
        "Tab:"
    }

    fn status_plain_text(&self) -> &str {
        "Texto Plano"
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
        "Diseño:"
    }

    fn status_panel(&self) -> &str {
        "Panel:"
    }

    // UI elements
    fn ui_yes(&self) -> &str {
        "Sí"
    }

    fn ui_no(&self) -> &str {
        "No"
    }

    fn ui_ok(&self) -> &str {
        "OK"
    }

    fn ui_cancel(&self) -> &str {
        "Cancelar"
    }

    fn ui_continue(&self) -> &str {
        "Continuar"
    }

    fn ui_close(&self) -> &str {
        "Cerrar"
    }

    fn ui_enter_confirm(&self) -> &str {
        "Enter - confirmar"
    }

    fn ui_esc_cancel(&self) -> &str {
        "Esc - cancelar"
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
        "Información del Archivo"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("Info del archivo '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("Info del directorio '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("Info del enlace simbólico '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "Nombre"
    }

    fn file_info_path(&self) -> &str {
        "Ruta"
    }

    fn file_info_type(&self) -> &str {
        "Tipo"
    }

    fn file_info_size(&self) -> &str {
        "Tamaño"
    }

    fn file_info_owner(&self) -> &str {
        "Propietario"
    }

    fn file_info_group(&self) -> &str {
        "Grupo"
    }

    fn file_info_created(&self) -> &str {
        "Creado"
    }

    fn file_info_modified(&self) -> &str {
        "Modificado"
    }

    fn file_info_calculating(&self) -> &str {
        "Calculando"
    }

    fn file_info_press_key(&self) -> &str {
        "Presione cualquier tecla para cerrar"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        if count == 0 {
            "sin cambios para confirmar".to_string()
        } else if count == 1 {
            "1 cambio para confirmar".to_string()
        } else {
            format!("{} cambios para confirmar", count)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        if count == 0 {
            "sin commits para enviar".to_string()
        } else if count == 1 {
            "1 commit para enviar".to_string()
        } else {
            format!("{} commits para enviar", count)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        if count == 0 {
            "sin commits para recibir".to_string()
        } else if count == 1 {
            "1 commit para recibir".to_string()
        } else {
            format!("{} commits para recibir", count)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "no está en el índice de git"
    }

    // File types
    fn file_type_directory(&self) -> &str {
        "Directorio"
    }

    fn file_type_file(&self) -> &str {
        "Archivo"
    }

    fn file_type_symlink(&self) -> &str {
        "Enlace Simbólico"
    }
}
