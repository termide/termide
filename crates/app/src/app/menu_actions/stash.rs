//! Stash menu actions — stash dropdown operations.

use anyhow::Result;

use super::super::App;
use crate::state::{ActiveModal, PendingAction};

impl App {
    /// Handle keyboard event in stash dropdown submenu
    pub(in crate::app) fn handle_stash_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use super::{navigate_submenu, SubmenuNavAction};

        // Item count depends on has_changes (controls "New stash..." visibility)
        let items = termide_ui_render::get_stash_items(
            &self.state.stash.entries,
            self.state.stash.has_changes,
        );
        let item_count = items.len();
        let separator_indices: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.is_separator)
            .map(|(idx, _)| idx)
            .collect();

        match navigate_submenu(
            &key,
            &mut self.state.ui.stash_submenu,
            item_count,
            &separator_indices,
        ) {
            SubmenuNavAction::Close => {
                self.state.ui.stash_submenu.close();
            }
            SubmenuNavAction::Execute => {
                self.execute_stash_submenu_action()?;
            }
            SubmenuNavAction::Left | SubmenuNavAction::Right => {
                self.state.ui.stash_submenu.close();
            }
            SubmenuNavAction::Delete => {
                self.delete_selected_stash()?;
            }
            SubmenuNavAction::Rename => {
                self.rename_selected_stash()?;
            }
            SubmenuNavAction::Edit | SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Rename the currently selected stash entry (change message via InputModal).
    fn rename_selected_stash(&mut self) -> Result<()> {
        let selected = self.state.ui.stash_submenu.selected;
        let items = termide_ui_render::get_stash_items(
            &self.state.stash.entries,
            self.state.stash.has_changes,
        );
        let item = match items.get(selected) {
            Some(i) if !i.is_separator && i.key != termide_ui_render::STASH_NEW => i,
            _ => return Ok(()),
        };
        if let Some(entry) = self
            .state
            .stash
            .entries
            .iter()
            .find(|e| e.ref_str == item.key)
        {
            let repo_path = match &self.state.stash.repo_path {
                Some(p) => p.clone(),
                None => return Ok(()),
            };
            let t = termide_i18n::t();
            self.state.ui.stash_submenu.close();
            let modal = termide_modal::InputModal::with_default(
                t.help_desc_rename(),
                t.help_desc_rename(),
                &entry.message,
            );
            self.state.set_pending_action(
                PendingAction::GitStashRename {
                    repo_path,
                    index: entry.index,
                },
                ActiveModal::Input(Box::new(modal)),
            );
        }
        Ok(())
    }

    /// Delete the currently selected stash entry (with confirmation).
    fn delete_selected_stash(&mut self) -> Result<()> {
        let selected = self.state.ui.stash_submenu.selected;
        let items = termide_ui_render::get_stash_items(
            &self.state.stash.entries,
            self.state.stash.has_changes,
        );
        let item = match items.get(selected) {
            Some(i) if !i.is_separator && i.key != termide_ui_render::STASH_NEW => i,
            _ => return Ok(()),
        };
        if let Some(entry) = self
            .state
            .stash
            .entries
            .iter()
            .find(|e| e.ref_str == item.key)
        {
            let repo_path = match &self.state.stash.repo_path {
                Some(p) => p.clone(),
                None => return Ok(()),
            };
            let t = termide_i18n::t();
            let modal = termide_modal::ConfirmModal::new(t.stash_drop(), &entry.message);
            self.state.ui.stash_submenu.close();
            self.state.set_pending_action(
                termide_state::PendingAction::GitStashDrop {
                    repo_path,
                    index: entry.index,
                },
                termide_modal::ActiveModal::Confirm(Box::new(modal)),
            );
        }
        Ok(())
    }

    /// Execute the currently selected stash submenu item.
    pub(in crate::app) fn execute_stash_submenu_action(&mut self) -> Result<()> {
        let selected = self.state.ui.stash_submenu.selected;
        let items = termide_ui_render::get_stash_items(
            &self.state.stash.entries,
            self.state.stash.has_changes,
        );
        let item = match items.get(selected) {
            Some(i) if !i.is_separator => i,
            _ => return Ok(()),
        };

        if item.key == termide_ui_render::STASH_NEW {
            // "New stash..." — open input modal
            let repo_path = match &self.state.stash.repo_path {
                Some(p) => p.clone(),
                None => {
                    self.state.ui.stash_submenu.close();
                    return Ok(());
                }
            };
            self.state.ui.stash_submenu.close();
            let t = termide_i18n::t();
            let modal = termide_modal::InputModal::new(t.stash_new(), "")
                .with_checkbox(t.stash_include_untracked().to_string());
            self.state.set_pending_action(
                PendingAction::GitStashPush { repo_path },
                ActiveModal::Input(Box::new(modal)),
            );
        } else {
            // Stash entry — find by ref_str and open info modal
            if let Some(stash_index) = self
                .state
                .stash
                .entries
                .iter()
                .position(|e| e.ref_str == item.key)
            {
                self.open_stash_info_modal(stash_index);
            }
        }
        Ok(())
    }

    /// Read a stash entry's details in the background; its window opens
    /// when they come.
    fn open_stash_info_modal(&mut self, stash_index: usize) {
        let repo_path = match &self.state.stash.repo_path {
            Some(p) => p.clone(),
            None => return,
        };
        let kind = crate::state::GitJobKind::StashInfo {
            index: stash_index,
            project_root: self.project_root.clone(),
        };
        self.spawn_git_job(&repo_path, kind, move |repo| {
            Ok(crate::state::GitJobOutput::StashDetails(
                termide_git::stash_info(repo, stash_index),
            ))
        });
    }

    /// Open the InfoActionModal on the details `info` of stash entry
    /// `stash_index`.
    pub(in crate::app) fn show_stash_info_modal(
        &mut self,
        repo_path: std::path::PathBuf,
        stash_index: usize,
        info: termide_git::StashInfo,
    ) {
        {
            let t = termide_i18n::t();
            let title = &info.message;

            let changes = format!(
                "{} {} (+{} -{})",
                info.files_changed,
                t.stash_files(),
                info.insertions,
                info.deletions
            );

            let mut lines = vec![
                (t.stash_created().to_string(), info.date),
                (t.stash_changes().to_string(), changes),
                (String::new(), String::new()),
            ];

            // Add file list (first 8 files)
            let max_files = 8;
            for (i, name) in info.file_names.iter().enumerate() {
                if i >= max_files {
                    let remaining = info.file_names.len() - max_files;
                    lines.push((
                        String::new(),
                        format!("...+{} {}", remaining, t.stash_more()),
                    ));
                    break;
                }
                lines.push((String::new(), format!(" {}", name)));
            }

            use termide_modal::{ActionButton, InfoActionModal};
            let buttons = vec![
                ActionButton::new(t.stash_apply(), "apply"),
                ActionButton::new(t.stash_pop(), "pop"),
                ActionButton::new(t.stash_drop(), "drop"),
                ActionButton::new(t.stash_diff(), "diff"),
                ActionButton::new(t.ui_close(), "close"),
            ];

            let modal = InfoActionModal::new(title, lines, buttons).with_selected_button(4);

            self.state.ui.stash_submenu.close();
            self.state.set_pending_action(
                termide_state::PendingAction::GitStashAction {
                    repo_path,
                    index: stash_index,
                    ref_str: format!("stash@{{{}}}", stash_index),
                },
                termide_modal::ActiveModal::InfoAction(Box::new(modal)),
            );
        }
    }
}
