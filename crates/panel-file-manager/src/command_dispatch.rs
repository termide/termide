//! File manager command dispatch: hotkey table construction and the
//! `execute_command` router that turns `FmCommand`s into panel events.

use std::path::PathBuf;

use termide_config::{Config, KeyBinding};
use termide_core::{HotkeyTable, PanelEvent};
use termide_git::GitStatus;
use termide_modal::{ActiveModal, ConfirmModal, InputModal};
use termide_state::PendingAction;
use termide_ui::{clipboard, path_utils};

use super::{keyboard, FileManager};

/// Build HotkeyTable for the file manager from config.
pub(crate) fn build_fm_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.file_manager.keybindings;

    // File operations
    t.insert("rename", &kb.rename);
    t.insert("view", &kb.view);
    t.insert("edit", &kb.edit);
    t.insert("copy", &kb.copy);
    t.insert("move", &kb.move_item);
    t.insert("create_dir", &kb.create_dir);
    t.insert("create_file", &kb.create_file);
    t.insert("delete", &kb.delete);
    t.insert("info", &kb.info);

    // Search
    t.insert("search", &kb.search);
    t.insert("search_content", &kb.search_content);
    t.insert("search_replace", &kb.search_replace);

    // Navigation
    t.insert("refresh", &kb.refresh);
    t.insert("go_parent", &kb.go_parent);
    t.insert("go_home", &kb.go_home);
    t.insert("toggle_selection", &kb.toggle_selection);
    t.insert("select_all", &kb.select_all);
    t.insert("toggle_hidden", &kb.toggle_hidden);

    // open_external: config binding, always ensure O present
    if let Some(ref binding) = kb.open_external {
        let mut keys: Vec<String> = match binding {
            KeyBinding::Single(s) => vec![s.clone()],
            KeyBinding::Multiple(v) => v.clone(),
        };
        if !keys.iter().any(|k| k == "O") {
            keys.push("O".into());
        }
        t.insert("open_external", &Some(KeyBinding::Multiple(keys)));
    } else {
        t.insert(
            "open_external",
            &Some(KeyBinding::Multiple(vec!["O".into(), "Alt+Enter".into()])),
        );
    }
    t.insert("switch_directory", &kb.switch_directory);
    t.insert("go_to_path", &kb.go_to_path);
    t
}

impl FileManager {
    /// Execute a file manager command and return resulting events.
    pub(crate) fn execute_command(&mut self, command: keyboard::FmCommand) -> Vec<PanelEvent> {
        use keyboard::FmCommand;

        let mut events = Vec::new();

        match command {
            // Navigation
            FmCommand::MoveUp => self.move_up(),
            FmCommand::MoveDown => self.move_down(),
            FmCommand::PageUp => {
                self.selected = self.selected.saturating_sub(self.visible_height);
            }
            FmCommand::PageDown => {
                let max_index = self.visible_count().saturating_sub(1);
                self.selected = (self.selected + self.visible_height).min(max_index);
            }
            FmCommand::GoHome => {
                self.selected = 0;
                self.scroll_offset = 0;
            }
            FmCommand::GoEnd => {
                self.selected = self.visible_count().saturating_sub(1);
            }
            FmCommand::Enter => {
                if let Some(event) = self.enter() {
                    events.push(event);
                }
            }
            FmCommand::GoParent => {
                // Use VfsState for navigation (works for both local and remote paths)
                // navigate_up returns None if already at root - don't refresh in that case
                if let Some(dir_name) = self.vfs.navigate_up() {
                    self.navigation.save_for_going_up(dir_name);
                    // Sync local path with VfsState
                    self.current_path = self.vfs.path_buf();
                    let _ = self.load_directory();
                }
            }
            FmCommand::GoHomeDir => {
                if let Some(home) = dirs::home_dir() {
                    self.current_path = std::fs::canonicalize(&home).unwrap_or(home);
                    let _ = self.load_directory();
                }
            }

            // Selection
            FmCommand::ToggleSelection => {
                self.toggle_selection();
            }
            FmCommand::SelectAll => self.select_all(),
            FmCommand::ClearSelection => {
                // If there's a pending VFS operation, cancel it instead of clearing selection
                if self.vfs.has_pending_operation() {
                    if let Some(message) = self.vfs.cancel_pending() {
                        // Sync FileManager path with VfsState
                        self.current_path = self.vfs.path_buf();
                        let _ = self.load_directory();
                        // Show cancellation modal
                        let t = termide_i18n::t();
                        self.show_info_modal(t.connection_cancelled_title(), &message);
                        events.push(PanelEvent::ClearStatus);
                    }
                } else {
                    self.selection.clear();
                }
            }
            FmCommand::CancelOperation => {
                // Explicitly cancel pending VFS operation
                if let Some(message) = self.vfs.cancel_pending() {
                    // Sync FileManager path with VfsState
                    self.current_path = self.vfs.path_buf();
                    let _ = self.load_directory();
                    // Show cancellation modal
                    let t = termide_i18n::t();
                    self.show_info_modal(t.connection_cancelled_title(), &message);
                    events.push(PanelEvent::ClearStatus);
                }
            }
            FmCommand::MoveUpWithSelection => self.move_up_with_selection(),
            FmCommand::MoveDownWithSelection => self.move_down_with_selection(),
            FmCommand::PageUpWithSelection => self.page_up_with_selection(),
            FmCommand::PageDownWithSelection => self.page_down_with_selection(),
            FmCommand::SelectToHome => self.select_to_home(),
            FmCommand::SelectToEnd => self.select_to_end(),
            FmCommand::MoveUpWithToggle => self.move_up_with_toggle(),
            FmCommand::MoveDownWithToggle => self.move_down_with_toggle(),
            FmCommand::PageUpWithToggle => self.page_up_with_toggle(),
            FmCommand::PageDownWithToggle => self.page_down_with_toggle(),

            // File operations
            FmCommand::NewFile => {
                let t = termide_i18n::t();
                let modal = InputModal::new(t.modal_create_file_title(), "");
                let action = PendingAction::CreateFile {
                    directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            FmCommand::NewDirectory => {
                let t = termide_i18n::t();
                let modal = InputModal::new(t.modal_create_dir_title(), "");
                let action = PendingAction::CreateDirectory {
                    directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }
            FmCommand::DeleteFiles => {
                if self.is_remote() {
                    // Remote delete - use VfsPath
                    let vfs_paths = self.get_selected_vfs_paths();
                    if !vfs_paths.is_empty() {
                        let t = termide_i18n::t();
                        let title = if vfs_paths.len() == 1 {
                            let file_name = vfs_paths[0]
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "file".to_string());
                            t.modal_delete_single_title(&file_name)
                        } else {
                            t.modal_delete_multiple_title(vfs_paths.len())
                        };
                        let modal = ConfirmModal::new(&title, "");
                        let action = PendingAction::DeleteRemotePath {
                            paths: vfs_paths,
                            vfs_manager: self.vfs_manager_arc(),
                        };
                        self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
                    }
                } else {
                    // Local delete - use PathBuf
                    let paths = self.get_selected_paths();
                    if !paths.is_empty() {
                        let t = termide_i18n::t();
                        let title = if paths.len() == 1 {
                            let file_name = path_utils::get_file_name_str(&paths[0]);
                            t.modal_delete_single_title(file_name)
                        } else {
                            t.modal_delete_multiple_title(paths.len())
                        };
                        let modal = ConfirmModal::new(&title, "");
                        let action = PendingAction::DeletePath { paths };
                        self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
                    }
                }
            }
            FmCommand::CopyFiles => {
                let paths = self.get_selected_paths();
                if !paths.is_empty() {
                    let t = termide_i18n::t();
                    let (message, default_dest) = if paths.len() == 1 {
                        let name = path_utils::get_file_name_str(&paths[0]);
                        // Single file: show full path with filename (user can rename)
                        (
                            t.fm_copy_prompt(name),
                            format!("{}/{}", self.current_path.display(), name),
                        )
                    } else {
                        // Multiple files: directory only (trailing slash)
                        (
                            format!("Copy {} items to:", paths.len()),
                            format!("{}/", self.current_path.display()),
                        )
                    };
                    let modal = InputModal::with_default("Copy", &message, &default_dest);
                    let action = PendingAction::CopyPath {
                        sources: paths,
                        target_directory: None,
                        create_symlink: false,
                        create_relative_symlink: false,
                    };
                    self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
                }
            }
            FmCommand::MoveFiles => {
                let paths = self.get_selected_paths();
                if !paths.is_empty() {
                    let t = termide_i18n::t();
                    let (message, default_dest) = if paths.len() == 1 {
                        let name = path_utils::get_file_name_str(&paths[0]);
                        (t.fm_move_prompt(name), name.to_string())
                    } else {
                        (
                            format!("Move {} items to:", paths.len()),
                            format!("{}/", self.current_path.display()),
                        )
                    };
                    let modal = InputModal::with_default("Move", &message, &default_dest);
                    let action = PendingAction::MovePath {
                        sources: paths,
                        target_directory: None,
                    };
                    self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
                }
            }
            FmCommand::RenameFile => {
                if let Some(te) = self.tree_entry_at(self.selected) {
                    let entry = &te.file_entry;
                    // Only allow renaming files and directories (not deleted or special entries)
                    if entry.git_status == GitStatus::Deleted {
                        return events;
                    }
                    let filename = entry.name.clone();
                    // For remote panels we must hand the operation layer a
                    // VFS URL pair, not a local PathBuf — otherwise the
                    // move falls through the `is_vfs_url` check and the
                    // file ends up renamed on the *local* filesystem (a
                    // very nasty surprise when local and remote share a
                    // path like /home/$USER).
                    let (source, target_dir) = if self.vfs.is_remote() {
                        let parent = self.vfs.current_path().clone();
                        let src_url = parent.join(&filename).to_url_string();
                        let parent_url = parent.to_url_string();
                        (PathBuf::from(src_url), Some(PathBuf::from(parent_url)))
                    } else {
                        let path = te.full_path.clone();
                        let parent = path.parent().map(|p| p.to_path_buf());
                        (path, parent)
                    };
                    let t = termide_i18n::t();
                    let modal = InputModal::with_default(
                        t.op_type_rename(),
                        t.fm_move_prompt(&filename),
                        &filename,
                    );
                    let action = PendingAction::MovePath {
                        sources: vec![source],
                        target_directory: target_dir,
                    };
                    self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
                }
            }
            FmCommand::EditFile => {
                if let Some(event) = self.edit_file() {
                    events.push(event);
                }
            }
            FmCommand::ViewFile => {
                if let Some(event) = self.view_file() {
                    events.push(event);
                }
            }
            FmCommand::OpenExternal => {
                if let Some(event) = self.open_external() {
                    events.push(event);
                }
            }

            // Search
            FmCommand::Search => {
                // File-name search is an inline bar docked in the panel.
                self.open_name_bar();
                events.push(PanelEvent::NeedsRedraw);
            }
            FmCommand::SearchContent => {
                // Content search is an inline bar docked in the panel.
                self.open_content_bar(false);
                events.push(PanelEvent::NeedsRedraw);
            }
            FmCommand::SearchReplace => {
                // Content replace: same inline bar with the Replace field.
                self.open_content_bar(true);
                events.push(PanelEvent::NeedsRedraw);
            }

            // Misc
            FmCommand::ShowFileInfo => self.show_file_info(),
            FmCommand::Refresh => {
                let _ = self.reload_directory();
            }
            FmCommand::ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                let _ = self.reload_directory();
            }
            FmCommand::NextPanel => {
                let modal = ConfirmModal::new("", "");
                self.modal_request = Some((
                    PendingAction::NextPanel,
                    ActiveModal::Confirm(Box::new(modal)),
                ));
            }
            FmCommand::PrevPanel => {
                let modal = ConfirmModal::new("", "");
                self.modal_request = Some((
                    PendingAction::PrevPanel,
                    ActiveModal::Confirm(Box::new(modal)),
                ));
            }
            FmCommand::GoToPath => {
                // Open input modal to enter path or URL (supports sftp://, ftp://, etc.)
                let t = termide_i18n::t();
                // Use directory at cursor position (may differ from panel root in tree view)
                let current_path = if let Some(te) = self.tree_entry_at(self.selected) {
                    if te.file_entry.is_dir {
                        te.full_path.display().to_string()
                    } else {
                        te.full_path
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| self.display_path())
                    }
                } else {
                    self.display_path()
                };
                let modal =
                    InputModal::with_default(t.fm_goto_title(), t.fm_goto_prompt(), &current_path);
                let action = PendingAction::GoToPath {
                    current_directory: self.current_path.clone(),
                };
                self.modal_request = Some((action, ActiveModal::Input(Box::new(modal))));
            }

            FmCommand::SwitchDirectory => {
                return vec![PanelEvent::OpenDirectorySwitcher];
            }

            // Tree expand/collapse
            FmCommand::ExpandDir => {
                if let Some(te) = self.tree_entry_at(self.selected) {
                    if te.expanded == Some(false) {
                        self.expand_dir(self.selected);
                    }
                }
            }
            FmCommand::CollapseDir => {
                // If current item is an expanded dir, collapse it
                // If current item is inside an expanded subtree, jump to parent dir
                if let Some(te) = self.tree_entry_at(self.selected) {
                    if te.expanded == Some(true) {
                        self.collapse_dir(self.selected);
                    } else if te.depth > 0 {
                        // Navigate up to parent directory in tree
                        self.jump_to_parent_dir();
                    }
                }
            }

            // No operation
            FmCommand::None => {}
        }

        events
    }

    /// Copy the selected item paths to the system clipboard as newline-joined
    /// paths (global `PanelCommand::Copy`).
    pub(crate) fn clipboard_copy_selection(&self) {
        let paths = self.get_selected_paths();
        if !paths.is_empty() {
            let text = paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let _ = clipboard::copy(&text);
        }
    }

    /// Mark the selected item paths for move on the system clipboard
    /// (global `PanelCommand::Cut`).
    pub(crate) fn clipboard_cut_selection(&self) {
        let paths = self.get_selected_paths();
        if !paths.is_empty() {
            let text = paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let _ = clipboard::cut(&text);
        }
    }

    /// Paste files referenced by the system clipboard into the cursor's tree
    /// level (global `PanelCommand::Paste`).
    pub(crate) fn clipboard_paste_files(&mut self) {
        if let Some(text) = clipboard::paste() {
            let files: Vec<PathBuf> = text
                .lines()
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.exists())
                .collect();

            if !files.is_empty() {
                // Land the paste at the cursor's tree level —
                // same rule as create_file / create_dir use via
                // `create_target_dir`. Cursor on a root entry
                // pastes into `current_path`; cursor inside an
                // expanded subdir pastes into that subdir.
                let (local_target, _vfs_target) = self.create_target_dir();
                let t = termide_i18n::t();
                let message =
                    t.fm_paste_confirm(files.len(), "Copy", &local_target.display().to_string());
                let action = PendingAction::CopyPath {
                    sources: files,
                    target_directory: Some(local_target),
                    create_symlink: false,
                    create_relative_symlink: false,
                };
                let modal = ConfirmModal::new(termide_i18n::t().modal_confirm_title(), &message);
                self.modal_request = Some((action, ActiveModal::Confirm(Box::new(modal))));
            }
        }
    }
}
