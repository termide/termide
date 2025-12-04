use super::Translation;

/// French translation
/// Traduction française
pub struct French;

impl Translation for French {
    fn fm_copy_files(&self) -> &str {
        "Fichiers copiés dans le presse-papiers"
    }

    fn fm_cut_files(&self) -> &str {
        "Fichiers coupés dans le presse-papiers"
    }

    fn fm_paste_confirm(&self, count: usize, mode: &str, dest: &str) -> String {
        format!(
            "{} {} fichier{} vers:\n{}",
            mode,
            count,
            if count <= 1 { "" } else { "s" },
            dest
        )
    }

    fn fm_delete_confirm(&self, count: usize) -> String {
        format!(
            "Supprimer {} fichier{} ?",
            count,
            if count <= 1 { "" } else { "s" }
        )
    }

    fn fm_rename_prompt(&self, old_name: &str) -> String {
        format!("Renommer '{}' en:", old_name)
    }

    fn fm_create_file_prompt(&self) -> &str {
        "Entrez le nom du fichier:"
    }

    fn fm_create_dir_prompt(&self) -> &str {
        "Entrez le nom du répertoire:"
    }

    fn fm_copy_prompt(&self, name: &str) -> String {
        format!("Copier '{}' vers:", name)
    }

    fn fm_move_prompt(&self, name: &str) -> String {
        format!("Déplacer '{}' vers:", name)
    }

    fn fm_search_prompt(&self) -> &str {
        "Rechercher:"
    }

    fn fm_no_results(&self) -> &str {
        "Aucun fichier correspondant trouvé"
    }

    fn fm_operation_cancelled(&self) -> &str {
        "Opération annulée"
    }

    fn modal_yes(&self) -> &str {
        "Oui"
    }

    fn modal_no(&self) -> &str {
        "Non"
    }

    fn modal_ok(&self) -> &str {
        "OK"
    }

    fn modal_cancel(&self) -> &str {
        "Annuler"
    }

    fn panel_file_manager(&self) -> &str {
        "Gestionnaire de fichiers"
    }

    fn panel_editor(&self, filename: &str) -> String {
        format!("Éditeur: {}", filename)
    }

    fn panel_terminal(&self) -> &str {
        "Terminal"
    }

    fn panel_welcome(&self) -> &str {
        "Bienvenue"
    }

    fn editor_close_unsaved(&self) -> &str {
        "Fermer l'éditeur"
    }

    fn editor_close_unsaved_question(&self) -> &str {
        "Le fichier contient des modifications non enregistrées. Que faire?"
    }

    fn editor_save_and_close(&self) -> &str {
        "Enregistrer et fermer"
    }

    fn editor_close_without_saving(&self) -> &str {
        "Fermer sans enregistrer"
    }

    fn editor_cancel(&self) -> &str {
        "Annuler"
    }

    fn editor_save_error(&self, error: &str) -> String {
        format!("Échec de l'enregistrement du fichier: {}", error)
    }

    fn editor_saved(&self, path: &str) -> String {
        format!("Fichier enregistré: {}", path)
    }

    fn editor_file_opened(&self, filename: &str) -> String {
        format!("Fichier '{}' ouvert", filename)
    }

    fn editor_search_title(&self) -> &str {
        "Rechercher"
    }

    fn editor_search_prompt(&self) -> &str {
        "Entrez la recherche:"
    }

    fn editor_replace_title(&self) -> &str {
        "Remplacer"
    }

    fn editor_replace_prompt(&self) -> &str {
        "Rechercher:"
    }

    fn editor_replace_with_prompt(&self) -> &str {
        "Remplacer par:"
    }

    fn editor_search_match_info(&self, current: usize, total: usize) -> String {
        format!("Correspondance {}/{}", current, total)
    }

    fn editor_search_no_matches(&self) -> &str {
        "Aucune correspondance"
    }

    fn editor_deletion_marker(&self, count: usize) -> String {
        format!(
            "{} ligne{} supprimée{}",
            count,
            if count <= 1 { "" } else { "s" },
            if count <= 1 { "" } else { "s" }
        )
    }

    fn terminal_exit_confirm(&self) -> &str {
        "Le processus est toujours en cours. Fermer le terminal?"
    }

    fn terminal_exited(&self, code: i32) -> String {
        format!("Le processus s'est terminé avec le code {}", code)
    }

    fn git_detected(&self) -> &str {
        "Git détecté et disponible"
    }

    fn git_not_found(&self) -> &str {
        "Git non trouvé - intégration git désactivée"
    }

    fn app_quit_confirm(&self) -> &str {
        "Il y a des modifications non enregistrées. Quitter quand même?"
    }

    fn error_operation_failed(&self, error: &str) -> String {
        format!("Opération échouée: {}", error)
    }

    fn error_file_exists(&self, path: &str) -> String {
        format!("Le fichier ou répertoire existe déjà: {}", path)
    }

    fn error_invalid_path(&self) -> &str {
        "Chemin invalide"
    }

    fn error_source_eq_dest(&self) -> &str {
        "La source et la destination sont identiques"
    }

    fn error_dest_is_subdir(&self) -> &str {
        "La destination est un sous-répertoire de la source"
    }

    fn help_title(&self) -> &str {
        "Aide"
    }

    fn help_app_title(&self) -> &str {
        "TermIDE - Aide"
    }

    fn help_version(&self) -> &str {
        "0.3.0"
    }

    fn help_global_keys(&self) -> &str {
        "RACCOURCIS GLOBAUX"
    }

    fn help_file_manager_keys(&self) -> &str {
        "GESTIONNAIRE DE FICHIERS"
    }

    fn help_editor_keys(&self) -> &str {
        "ÉDITEUR DE TEXTE"
    }

    fn help_terminal_keys(&self) -> &str {
        "TERMINAL"
    }

    fn help_git_integration(&self) -> &str {
        "INTÉGRATION GIT"
    }

    fn help_clipboard_operations(&self) -> &str {
        "OPÉRATIONS PRESSE-PAPIERS"
    }

    fn help_close_hint(&self) -> &str {
        "Appuyez sur Esc ou Ctrl+H pour fermer cette fenêtre"
    }

    fn help_desc_menu(&self) -> &str {
        "Ouvrir/fermer le menu"
    }

    fn help_desc_panel_menu(&self) -> &str {
        "Menu du panneau"
    }

    fn help_desc_quit(&self) -> &str {
        "Quitter l'application"
    }

    fn help_desc_help(&self) -> &str {
        "Afficher l'aide"
    }

    fn help_desc_switch_panel(&self) -> &str {
        "Basculer entre les panneaux"
    }

    fn help_desc_close_panel(&self) -> &str {
        "Fermer le panneau actif"
    }

    fn help_desc_navigate(&self) -> &str {
        "Naviguer dans la liste"
    }

    fn help_desc_select(&self) -> &str {
        "Basculer la sélection"
    }

    fn help_desc_select_all(&self) -> &str {
        "Sélectionner tous les fichiers"
    }

    fn help_desc_open_file(&self) -> &str {
        "Ouvrir le répertoire ou fichier dans l'éditeur"
    }

    fn help_desc_open_editor(&self) -> &str {
        "Ouvrir le fichier dans l'éditeur"
    }

    fn help_desc_new_terminal(&self) -> &str {
        "Ouvrir un nouveau terminal dans le répertoire actuel"
    }

    fn help_desc_parent_dir(&self) -> &str {
        "Aller au répertoire parent"
    }

    fn help_desc_home(&self) -> &str {
        "Aller au début de la liste"
    }

    fn help_desc_end(&self) -> &str {
        "Aller à la fin de la liste"
    }

    fn help_desc_page_scroll(&self) -> &str {
        "Défiler par pages"
    }

    fn help_desc_create_file(&self) -> &str {
        "Créer un nouveau fichier"
    }

    fn help_desc_create_dir(&self) -> &str {
        "Créer un nouveau répertoire"
    }

    fn help_desc_rename(&self) -> &str {
        "Renommer le fichier/répertoire"
    }

    fn help_desc_copy(&self) -> &str {
        "Copier le fichier/répertoire"
    }

    fn help_desc_move(&self) -> &str {
        "Déplacer le fichier/répertoire"
    }

    fn help_desc_delete(&self) -> &str {
        "Supprimer le fichier/répertoire sélectionné"
    }

    fn help_desc_search(&self) -> &str {
        "Rechercher des fichiers"
    }

    fn help_desc_fm_copy_clipboard(&self) -> &str {
        "Copier les fichiers sélectionnés dans le presse-papiers"
    }

    fn help_desc_fm_cut_clipboard(&self) -> &str {
        "Couper les fichiers sélectionnés dans le presse-papiers"
    }

    fn help_desc_fm_paste_clipboard(&self) -> &str {
        "Coller les fichiers du presse-papiers"
    }

    fn help_desc_save(&self) -> &str {
        "Enregistrer le fichier"
    }

    fn help_desc_copy_system(&self) -> &str {
        "Copier dans le presse-papiers système"
    }

    fn help_desc_paste_system(&self) -> &str {
        "Coller du presse-papiers système"
    }

    fn help_desc_cut_system(&self) -> &str {
        "Couper dans le presse-papiers système"
    }

    fn help_desc_paste_ctrl_y(&self) -> &str {
        "Coller du presse-papiers système"
    }

    fn help_desc_undo(&self) -> &str {
        "Annuler"
    }

    fn help_desc_redo(&self) -> &str {
        "Refaire"
    }

    fn help_desc_git_ignored(&self) -> &str {
        "Gris foncé - ignoré par git"
    }

    fn help_desc_git_modified(&self) -> &str {
        "Jaune - modifié"
    }

    fn help_desc_git_added(&self) -> &str {
        "Vert - nouveau/ajouté"
    }

    fn help_desc_git_deleted(&self) -> &str {
        "Rouge pâle - supprimé (lecture seule)"
    }

    fn status_file_created(&self, name: &str) -> String {
        format!("Fichier '{}' créé", name)
    }

    fn status_error_create_file(&self, error: &str) -> String {
        format!("Erreur de création de fichier: {}", error)
    }

    fn status_dir_created(&self, name: &str) -> String {
        format!("Répertoire '{}' créé", name)
    }

    fn status_error_create_dir(&self, error: &str) -> String {
        format!("Erreur de création de répertoire: {}", error)
    }

    fn status_item_deleted(&self) -> &str {
        "Élément supprimé"
    }

    fn status_error_delete(&self) -> &str {
        "Erreur de suppression"
    }

    fn status_items_deleted(&self, count: usize) -> String {
        format!("{} éléments supprimés", count)
    }

    fn status_items_deleted_with_errors(&self, success: usize, errors: usize) -> String {
        format!("Supprimés: {}, erreurs: {}", success, errors)
    }

    fn status_file_saved(&self, name: &str) -> String {
        format!("Fichier '{}' enregistré", name)
    }

    fn status_error_save(&self, error: &str) -> String {
        format!("Erreur d'enregistrement: {}", error)
    }

    fn status_error_open_file(&self, name: &str, error: &str) -> String {
        format!("Erreur d'ouverture de '{}': {}", name, error)
    }

    fn status_item_actioned(&self, name: &str, action: &str) -> String {
        format!("'{}' {}", name, action)
    }

    fn status_error_action(&self, action: &str, error: &str) -> String {
        format!("Erreur {}: {}", action, error)
    }

    fn status_operation_skipped(&self, name: &str) -> String {
        format!("Opération '{}' ignorée", name)
    }

    fn action_copied(&self) -> &str {
        "copié"
    }

    fn action_moved(&self) -> &str {
        "déplacé"
    }

    fn action_copying(&self) -> &str {
        "copie"
    }

    fn action_moving(&self) -> &str {
        "déplacement"
    }

    fn modal_copy_single_title(&self, name: &str) -> String {
        format!("Copier '{}'", name)
    }

    fn modal_copy_multiple_title(&self, count: usize) -> String {
        format!("Copier {} éléments", count)
    }

    fn modal_move_single_title(&self, name: &str) -> String {
        format!("Déplacer '{}'", name)
    }

    fn modal_move_multiple_title(&self, count: usize) -> String {
        format!("Déplacer {} éléments", count)
    }

    fn modal_create_file_title(&self) -> &str {
        "Créer un fichier"
    }

    fn modal_create_dir_title(&self) -> &str {
        "Créer un répertoire"
    }

    fn modal_delete_single_title(&self, name: &str) -> String {
        format!("Supprimer '{}'", name)
    }

    fn modal_delete_multiple_title(&self, count: usize) -> String {
        format!("Supprimer {} éléments", count)
    }

    fn modal_save_as_title(&self) -> &str {
        "Enregistrer sous"
    }

    fn modal_enter_filename(&self) -> &str {
        "Entrez le nom du fichier:"
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
        "copié"
    }

    fn batch_result_file_moved(&self) -> &str {
        "déplacé"
    }

    fn batch_result_error_copy(&self) -> &str {
        "Erreur de copie"
    }

    fn batch_result_error_move(&self) -> &str {
        "Erreur de déplacement"
    }

    fn batch_result_copied(&self) -> &str {
        "Copié"
    }

    fn batch_result_moved(&self) -> &str {
        "Déplacé"
    }

    fn batch_result_skipped_fmt(&self, count: usize) -> String {
        format!("ignorés: {}", count)
    }

    fn batch_result_errors_fmt(&self, count: usize) -> String {
        format!("erreurs: {}", count)
    }

    fn menu_files(&self) -> &str {
        "Fichiers"
    }

    fn menu_terminal(&self) -> &str {
        "Terminal"
    }

    fn menu_editor(&self) -> &str {
        "Éditeur"
    }

    fn menu_debug(&self) -> &str {
        "Journal"
    }

    fn menu_preferences(&self) -> &str {
        "Préférences"
    }

    fn menu_help(&self) -> &str {
        "Aide"
    }

    fn menu_quit(&self) -> &str {
        "Quitter"
    }

    fn menu_navigate_hint(&self) -> &str {
        "←→ Naviguer | Enter Sélectionner | Esc Fermer"
    }

    fn menu_open_hint(&self) -> &str {
        "Alt+M Menu"
    }

    fn status_dir(&self) -> &str {
        "Rép:"
    }

    fn status_file(&self) -> &str {
        "Fichier:"
    }

    fn status_mod(&self) -> &str {
        "Mod:"
    }

    fn status_owner(&self) -> &str {
        "Propriétaire:"
    }

    fn status_selected(&self) -> &str {
        "Sélectionnés:"
    }

    fn status_pos(&self) -> &str {
        "Pos:"
    }

    fn status_tab(&self) -> &str {
        "Tab:"
    }

    fn status_plain_text(&self) -> &str {
        "Texte brut"
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
        "Disposition:"
    }

    fn status_panel(&self) -> &str {
        "Panneau:"
    }

    fn ui_yes(&self) -> &str {
        "Oui"
    }

    fn ui_no(&self) -> &str {
        "Non"
    }

    fn ui_ok(&self) -> &str {
        "OK"
    }

    fn ui_cancel(&self) -> &str {
        "Annuler"
    }

    fn ui_continue(&self) -> &str {
        "Continuer"
    }

    fn ui_close(&self) -> &str {
        "Fermer"
    }

    fn ui_enter_confirm(&self) -> &str {
        "Enter - confirmer"
    }

    fn ui_esc_cancel(&self) -> &str {
        "Esc - annuler"
    }

    fn ui_hint_separator(&self) -> &str {
        " | "
    }

    fn size_bytes(&self) -> &str {
        "o"
    }

    fn size_kilobytes(&self) -> &str {
        "Ko"
    }

    fn size_megabytes(&self) -> &str {
        "Mo"
    }

    fn size_gigabytes(&self) -> &str {
        "Go"
    }

    fn size_terabytes(&self) -> &str {
        "To"
    }

    fn file_info_title(&self) -> &str {
        "Informations fichier"
    }

    fn file_info_title_file(&self, name: &str) -> String {
        format!("Info fichier '{}'", name)
    }

    fn file_info_title_directory(&self, name: &str) -> String {
        format!("Info répertoire '{}'", name)
    }

    fn file_info_title_symlink(&self, name: &str) -> String {
        format!("Info lien symbolique '{}'", name)
    }

    fn file_info_name(&self) -> &str {
        "Nom"
    }

    fn file_info_path(&self) -> &str {
        "Chemin"
    }

    fn file_info_type(&self) -> &str {
        "Type"
    }

    fn file_info_size(&self) -> &str {
        "Taille"
    }

    fn file_info_owner(&self) -> &str {
        "Propriétaire"
    }

    fn file_info_group(&self) -> &str {
        "Groupe"
    }

    fn file_info_created(&self) -> &str {
        "Créé"
    }

    fn file_info_modified(&self) -> &str {
        "Modifié"
    }

    fn file_info_calculating(&self) -> &str {
        "Calcul en cours"
    }

    fn file_info_press_key(&self) -> &str {
        "Appuyez sur n'importe quelle touche pour fermer"
    }

    fn file_info_git(&self) -> &str {
        "Git"
    }

    fn file_info_git_uncommitted(&self, count: usize) -> String {
        if count == 0 {
            "aucun changement à valider".to_string()
        } else if count == 1 {
            "1 changement à valider".to_string()
        } else {
            format!("{} changements à valider", count)
        }
    }

    fn file_info_git_ahead(&self, count: usize) -> String {
        if count == 0 {
            "aucun commit à pousser".to_string()
        } else if count == 1 {
            "1 commit à pousser".to_string()
        } else {
            format!("{} commits à pousser", count)
        }
    }

    fn file_info_git_behind(&self, count: usize) -> String {
        if count == 0 {
            "aucun commit à tirer".to_string()
        } else if count == 1 {
            "1 commit à tirer".to_string()
        } else {
            format!("{} commits à tirer", count)
        }
    }

    fn file_info_git_ignored(&self) -> &str {
        "pas dans l'index git"
    }

    fn file_type_directory(&self) -> &str {
        "Répertoire"
    }

    fn file_type_file(&self) -> &str {
        "Fichier"
    }

    fn file_type_symlink(&self) -> &str {
        "Lien symbolique"
    }
}
