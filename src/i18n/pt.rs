use super::Translation;

/// Portuguese (Brazilian) translation
/// Tradução em português (brasileiro)
pub struct Portuguese;

impl Translation for Portuguese {
    fn fm_copy_files(&self) -> &str {
        "Arquivos copiados para a área de transferência"
    }

    fn fm_cut_files(&self) -> &str {
        "Arquivos recortados para a área de transferência"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        format!(
            "{} {} arquivo{} para:\n{}",
            mode,
            count,
            if count == 1 { "" } else { "s" },
            dest
        )
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!(
            "Excluir {} arquivo{}?",
            count,
            if count == 1 { "" } else { "s" }
        )
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("Renomear '{}' para:", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "Digite o nome do arquivo:"
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "Digite o nome do diretório:"
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("Copiar '{}' para:", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("Mover '{}' para:", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "Pesquisar:"
    }

    fn fm_no_results(&self) -> &str {
        "Nenhum arquivo correspondente encontrado"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "Operação cancelada"
    }

    fn modal_yes(&self) -> &str {
        "Sim"
    }

    fn modal_no(&self) -> &str {
        "Não"
    }

    fn modal_ok(&self) -> &str {
        "OK"
    }

    fn modal_cancel(&self) -> &str {
        "Cancelar"
    }

    fn panel_file_manager(&self) -> &str {
        "Gerenciador de Arquivos"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("Editor: {}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "Terminal"
    }

    fn panel_welcome(&self) -> &str {
        "Bem-vindo"
    }

    fn editor_close_unsaved(&self) -> &str {
        "Fechar Editor"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "O arquivo tem alterações não salvas. O que fazer?"
    }

    fn editor_save_and_close(&self) -> &str {
        "Salvar e fechar"
    }

    fn editor_close_without_saving(&self) -> &str {
        "Fechar sem salvar"
    }

    fn editor_cancel(&self) -> &str {
        "Cancelar"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("Falha ao salvar arquivo: {}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("Arquivo salvo: {}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("Arquivo '{}' aberto", filename)
    }

    fn editor_search_title(&self) -> &str {
        "Pesquisar"
    }

    fn editor_search_prompt(&self) -> &str {
        "Digite a pesquisa:"
    }

    fn editor_replace_title(&self) -> &str {
        "Substituir"
    }

    fn editor_replace_prompt(&self) -> &str {
        "Pesquisar por:"
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "Substituir por:"
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("Correspondência {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "Nenhuma correspondência"
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        format!(
            "{} linha{} excluída{}",
            count,
            if count == 1 { "" } else { "s" },
            if count == 1 { "" } else { "s" }
        )
    }

    fn terminal_exit_confirm(&self) -> &str {
        "O processo ainda está em execução. Fechar terminal?"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("Processo encerrado com código {}", code)
    }

    fn git_detected(&self) -> &str {
        "Git detectado e disponível"
    }

    fn git_not_found(&self) -> &str {
        "Git não encontrado - integração git desabilitada"
    }

    fn app_quit_confirm(&self) -> &str {
        "Há alterações não salvas. Sair mesmo assim?"
    }

    fn error_operation_failed(&self, error: &str) -> String {
        format!("Operação falhou: {}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("Arquivo ou diretório já existe: {}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "Caminho inválido"
    }

    fn error_source_eq_dest(&self) -> &str {
        "Origem e destino são iguais"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "Destino é um subdiretório da origem"
    }

    fn help_title(&self) -> &str {
        "Ajuda"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - Ajuda"
    }

    fn help_version(&self) -> &str {
        "0.3.0"
    }

    fn help_global_keys(&self) -> &str {
        "ATALHOS GLOBAIS"
    }

    fn help_file_manager_keys(&self) -> &str {
        "GERENCIADOR DE ARQUIVOS"
    }

    fn help_editor_keys(&self) -> &str {
        "EDITOR DE TEXTO"
    }

    fn help_terminal_keys(&self) -> &str {
        "TERMINAL"
    }

    fn help_git_integration(&self) -> &str {
        "INTEGRAÇÃO GIT"
    }

    fn help_clipboard_operations(&self) -> &str {
        "OPERAÇÕES DE ÁREA DE TRANSFERÊNCIA"
    }

    fn help_close_hint(&self) -> &str {
        "Pressione Esc ou Ctrl+H para fechar esta janela"
    }

    fn help_desc_menu(&self) -> &str {
        "Abrir/fechar menu"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "Menu do painel"
    }

    fn help_desc_quit(&self) -> &str {
        "Sair da aplicação"
    }

    fn help_desc_help(&self) -> &str {
        "Mostrar ajuda"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "Alternar entre painéis"
    }

    fn help_desc_close_panel(&self) -> &str {
        "Fechar painel ativo"
    }

    fn help_desc_navigate(&self) -> &str {
        "Navegar pela lista"
    }

    fn help_desc_select(&self) -> &str {
        "Alternar seleção"
    }

    fn help_desc_select_all(&self) -> &str {
        "Selecionar todos os arquivos"
    }

    fn help_desc_open_file(&self) -> &str {
        "Abrir diretório ou arquivo no editor"
    }

    fn help_desc_open_editor(&self) -> &str {
        "Abrir arquivo no editor"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "Abrir novo terminal no diretório atual"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "Ir para o diretório pai"
    }

    fn help_desc_home(&self) -> &str {
        "Ir para o início da lista"
    }

    fn help_desc_end(&self) -> &str {
        "Ir para o final da lista"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "Rolar por páginas"
    }

    fn help_desc_create_file(&self) -> &str {
        "Criar novo arquivo"
    }

    fn help_desc_create_dir(&self) -> &str {
        "Criar novo diretório"
    }

    fn help_desc_rename(&self) -> &str {
        "Renomear arquivo/diretório"
    }

    fn help_desc_copy(&self) -> &str {
        "Copiar arquivo/diretório"
    }

    fn help_desc_move(&self) -> &str {
        "Mover arquivo/diretório"
    }

    fn help_desc_delete(&self) -> &str {
        "Excluir arquivo/diretório selecionado"
    }

    fn help_desc_search(&self) -> &str {
        "Pesquisar arquivos"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "Copiar arquivos selecionados para área de transferência"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "Recortar arquivos selecionados para área de transferência"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "Colar arquivos da área de transferência"
    }

    fn help_desc_save(&self) -> &str {
        "Salvar arquivo"
    }

    fn help_desc_copy_system(&self) -> &str {
        "Copiar para área de transferência do sistema"
    }

    fn help_desc_paste_system(&self) -> &str {
        "Colar da área de transferência do sistema"
    }

    fn help_desc_cut_system(&self) -> &str {
        "Recortar para área de transferência do sistema"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "Colar da área de transferência do sistema"
    }

    fn help_desc_undo(&self) -> &str {
        "Desfazer"
    }

    fn help_desc_redo(&self) -> &str {
        "Refazer"
    }

    fn help_desc_git_ignored(&self) -> &str {
        "Cinza escuro - ignorado pelo git"
    }

    fn help_desc_git_modified(&self) -> &str {
        "Amarelo - modificado"
    }

    fn help_desc_git_added(&self) -> &str {
        "Verde - novo/adicionado"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "Vermelho claro - excluído (somente leitura)"
    }

    fn status_file_created(&self, name: &str) -> String {
        format!("Arquivo '{}' criado", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("Erro ao criar arquivo: {}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("Diretório '{}' criado", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("Erro ao criar diretório: {}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "Item excluído"
    }

    fn status_error_delete(&self) -> &str {
        "Erro ao excluir"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("{} itens excluídos", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("Excluídos: {}, erros: {}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("Arquivo '{}' salvo", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("Erro ao salvar: {}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("Erro ao abrir '{}': {}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("Erro {}: {}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("Operação '{}' ignorada", name)
    }

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
        "movendo"
    }

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
        "Criar Arquivo"
    }

    fn modal_create_dir_title(&self) -> &str {
        "Criar Diretório"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("Excluir '{}'", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("Excluir {} elementos", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "Salvar Como"
    }

    fn modal_enter_filename(&self) -> &str {
        "Digite o nome do arquivo:"
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
        "copiado"
    }

    fn batch_result_file_moved(&self) -> &str {
        "movido"
    }

    fn batch_result_error_copy(&self) -> &str {
        "Erro ao copiar"
    }

    fn batch_result_error_move(&self) -> &str {
        "Erro ao mover"
    }

    fn batch_result_copied(&self) -> &str {
        "Copiado"
    }

    fn batch_result_moved(&self) -> &str {
        "Movido"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("ignorados: {}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("erros: {}", count)
    }

    fn menu_files(&self) -> &str {
        "Arquivos"
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
        "Preferências"
    }

    fn menu_help(&self) -> &str {
        "Ajuda"
    }

    fn menu_quit(&self) -> &str {
        "Sair"
    }

    fn menu_navigate_hint(&self) -> &str {
        "←→ Navegar | Enter Selecionar | Esc Fechar"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M Menu"
    }

    fn status_dir(&self) -> &str {
        "Dir:"
    }

    fn status_file(&self) -> &str {
        "Arquivo:"
    }

    fn status_mod(&self) -> &str {
        "Mod:"
    }

    fn status_owner(&self) -> &str {
        "Proprietário:"
    }

    fn status_selected(&self) -> &str {
        "Selecionados:"
    }

    fn status_pos(&self) -> &str {
        "Pos:"
    }

    fn status_tab(&self) -> &str {
        "Tab:"
    }

    fn status_plain_text(&self) -> &str {
        "Texto Simples"
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
        "Painel:"
    }

    fn ui_yes(&self) -> &str {
        "Sim"
    }

    fn ui_no(&self) -> &str {
        "Não"
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
        "Fechar"
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
        "Informações do Arquivo"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("Info do arquivo '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("Info do diretório '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("Info do link simbólico '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "Nome"
    }

    fn file_info_path(&self) -> &str {
        "Caminho"
    }

    fn file_info_type(&self) -> &str {
        "Tipo"
    }

    fn file_info_size(&self) -> &str {
        "Tamanho"
    }

    fn file_info_owner(&self) -> &str {
        "Proprietário"
    }

    fn file_info_group(&self) -> &str {
        "Grupo"
    }

    fn file_info_created(&self) -> &str {
        "Criado"
    }

    fn file_info_modified(&self) -> &str {
        "Modificado"
    }

    fn file_info_calculating(&self) -> &str {
        "Calculando"
    }

    fn file_info_press_key(&self) -> &str {
        "Pressione qualquer tecla para fechar"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        if count == 0 {
            "nenhuma alteração para confirmar".to_string()
        } else if count == 1 {
            "1 alteração para confirmar".to_string()
        } else {
            format!("{} alterações para confirmar", count)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        if count == 0 {
            "nenhum commit para enviar".to_string()
        } else if count == 1 {
            "1 commit para enviar".to_string()
        } else {
            format!("{} commits para enviar", count)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        if count == 0 {
            "nenhum commit para receber".to_string()
        } else if count == 1 {
            "1 commit para receber".to_string()
        } else {
            format!("{} commits para receber", count)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "não está no índice git"
    }

    fn file_type_directory(&self) -> &str {
        "Diretório"
    }

    fn file_type_file(&self) -> &str {
        "Arquivo"
    }

    fn file_type_symlink(&self) -> &str {
        "Link Simbólico"
    }
}
