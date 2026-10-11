//! Unpack: the destination prompt, its answer, and the confirmation before
//! unpacking over existing files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::super::App;
use crate::state::{ActiveModal, OperationType, PendingAction};
use termide_file_ops::{unpack_dir_name, unpack_targets, OperationRequest};
use termide_state::UnpackPlan;
use termide_ui::path_utils;

impl App {
    /// Build the destination prompt for unpacking `archives`.
    ///
    /// The default is the nearest other file manager's directory, as for
    /// copy, falling back to the archive's own. A single archive is offered
    /// a directory named after it in each candidate as well as the candidate
    /// itself; several archives always get a directory each, so only the
    /// parent is asked for.
    pub(in crate::app) fn prepare_unpack_modal(&mut self, archives: &[PathBuf]) -> ActiveModal {
        self.state.close_modal();
        let t = termide_i18n::t();

        let source_dir = archives[0]
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/".to_string());
        let local = |dir: &String| !termide_vfs::is_vfs_url(dir);
        let file_managers: Vec<String> = self
            .other_file_manager_paths()
            .into_iter()
            .filter(local)
            .collect();
        let panel_dirs: Vec<String> = self
            .find_all_other_panel_paths()
            .into_iter()
            .map(|opt| opt.value)
            .filter(local)
            .collect();
        let default_dir = file_managers
            .iter()
            .chain(&panel_dirs)
            .find(|dir| dir.trim_end_matches('/') != source_dir.trim_end_matches('/'))
            .cloned()
            .unwrap_or_else(|| source_dir.clone());

        let mut dirs: Vec<String> = Vec::new();
        for dir in std::iter::once(&default_dir)
            .chain(&panel_dirs)
            .chain(std::iter::once(&source_dir))
        {
            let dir = dir.trim_end_matches('/').to_string();
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }

        let (prompt, subdir) = match archives {
            [single] => (
                t.fm_unpack_prompt(&single.file_name().unwrap_or_default().to_string_lossy()),
                Some(unpack_dir_name(single)),
            ),
            many => (t.fm_unpack_prompt_multiple(many.len()), None),
        };
        let mut values: Vec<String> = Vec::new();
        for dir in &dirs {
            if let Some(name) = &subdir {
                values.push(format!("{dir}/{name}/"));
            }
            values.push(format!("{dir}/"));
        }
        let default = values[0].clone();
        let options = values
            .into_iter()
            .map(|value| termide_modal::SelectOption {
                display: value.clone(),
                value,
            })
            .collect();
        ActiveModal::EditableSelect(Box::new(termide_modal::EditableSelectModal::new(
            t.modal_unpack_title(),
            prompt,
            default,
            options,
        )))
    }

    /// Answer to the destination prompt. A relative path is taken from the
    /// archive's directory. A plain file in the way is refused here; the
    /// passwords and the confirmation follow in [`Self::continue_unpack`].
    pub(in crate::app) fn handle_unpack_archives(
        &mut self,
        archives: Vec<PathBuf>,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        let Some(input) = value.downcast_ref::<String>() else {
            return Ok(());
        };
        let input = input.trim();
        if input.is_empty() || archives.is_empty() {
            return Ok(());
        }
        let base = archives[0].parent().unwrap_or(Path::new("/"));
        let (destination, _) = path_utils::resolve_local_destination_input(base, input);

        if let Some((_, file)) = unpack_targets(&archives, &destination)
            .iter()
            .find(|(_, dest)| dest.symlink_metadata().is_ok() && !dest.is_dir())
        {
            let t = termide_i18n::t();
            self.show_error_modal(t.fm_pack_exists(&file.display().to_string()));
            return Ok(());
        }
        self.continue_unpack(UnpackPlan {
            archives,
            destination,
            passwords: Default::default(),
            checked: 0,
            confirmed: false,
        });
        Ok(())
    }

    /// Take the plan one step further: ask for the password of the next
    /// encrypted archive, then confirm replacing existing files, then start.
    ///
    /// Only zip archives can be encrypted; checking one reads its central
    /// directory and nothing more, so this runs on the spot. An archive that
    /// fails to open for another reason is left for the run to report.
    fn continue_unpack(&mut self, mut plan: UnpackPlan) {
        while let Some(archive) = plan.archives.get(plan.checked) {
            if matches!(
                termide_vfs::archive::check_password(archive, None),
                Err(termide_vfs::VfsError::PasswordRequired)
            ) {
                self.ask_unpack_password(plan, false);
                return;
            }
            plan.checked += 1;
        }
        if !plan.confirmed {
            if let Some((_, occupied)) = unpack_targets(&plan.archives, &plan.destination)
                .iter()
                .find(|(_, dest)| dir_has_entries(dest))
            {
                let t = termide_i18n::t();
                let modal = termide_modal::ConfirmModal::new(
                    t.modal_unpack_title(),
                    t.fm_unpack_not_empty(&occupied.display().to_string()),
                );
                self.state.set_pending_action(
                    PendingAction::ConfirmUnpack { plan },
                    ActiveModal::Confirm(Box::new(modal)),
                );
                return;
            }
        }
        self.start_unpack(plan);
    }

    /// Ask for the password of `plan.archives[plan.checked]`; `wrong` says
    /// the last one was rejected.
    fn ask_unpack_password(&mut self, plan: UnpackPlan, wrong: bool) {
        let t = termide_i18n::t();
        let name = plan.archives[plan.checked]
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prompt = if wrong {
            t.fm_archive_password_wrong(&name)
        } else {
            t.fm_archive_password_prompt(&name)
        };
        let modal =
            termide_modal::InputModal::new(t.modal_archive_password_title(), &prompt).password();
        self.state.set_pending_action(
            PendingAction::UnpackPassword { plan },
            ActiveModal::Input(Box::new(modal)),
        );
    }

    /// Answer to the password prompt: keep a password the archive accepts
    /// and go on, ask again after a wrong one. Cancelling drops the unpack.
    pub(in crate::app) fn handle_unpack_password(
        &mut self,
        mut plan: UnpackPlan,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        let Some(password) = value.downcast_ref::<String>() else {
            return Ok(());
        };
        let Some(archive) = plan.archives.get(plan.checked).cloned() else {
            return Ok(());
        };
        match termide_vfs::archive::check_password(&archive, Some(password)) {
            Ok(()) => {
                plan.passwords.insert(archive, password.clone());
                plan.checked += 1;
                self.continue_unpack(plan);
            }
            Err(termide_vfs::VfsError::WrongPassword | termide_vfs::VfsError::PasswordRequired) => {
                self.ask_unpack_password(plan, true);
            }
            Err(e) => {
                let t = termide_i18n::t();
                let name = archive
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.show_error_modal(
                    t.op_unpack_failed(&name, &termide_file_ops::vfs_error_text(&e)),
                );
            }
        }
        Ok(())
    }

    /// Answer to the "not empty" confirmation.
    pub(in crate::app) fn handle_confirm_unpack(
        &mut self,
        mut plan: UnpackPlan,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        if value.downcast_ref::<bool>().copied().unwrap_or(false) {
            plan.confirmed = true;
            self.continue_unpack(plan);
        }
        Ok(())
    }

    fn start_unpack(&mut self, plan: UnpackPlan) {
        let source_display = match &plan.archives[..] {
            [single] => single.display().to_string(),
            many => termide_i18n::t().status_item_count(many.len()),
        };
        let destination = plan.destination.display().to_string();
        let request = OperationRequest::unpack(plan.archives, plan.destination)
            .with_archive_passwords(plan.passwords);
        if let Err(e) = self.start_tracked_operation(
            request,
            Arc::new(termide_vfs::VfsManager::new()),
            OperationType::Unpack,
            source_display,
            destination,
            0,
            0,
        ) {
            log::error!("Failed to start unpack operation: {e}");
            self.show_error_modal(e.to_string());
        }
    }
}

/// Whether `dir` is a directory with at least one entry.
fn dir_has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::dir_has_entries;

    #[test]
    fn only_a_directory_with_entries_counts_as_occupied() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!dir_has_entries(dir.path()));
        assert!(!dir_has_entries(&dir.path().join("missing")));
        std::fs::write(dir.path().join("a"), "").unwrap();
        assert!(dir_has_entries(dir.path()));
    }
}
