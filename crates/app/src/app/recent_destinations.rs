//! The quick destinations of the path prompts (copy, move, pack, unpack,
//! Save As), among them the destinations recently confirmed, per project.
//! Those are kept for this run only: nothing is written to disk.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use super::App;
use crate::state::{ActiveModal, PendingAction};
use termide_modal::{DestinationKind, ModalResult, QuickDestination, SaveAsResult};
use termide_ui::path_utils;

/// How many destinations a project keeps.
const MAX_RECENT: usize = 10;

/// Directories confirmed as destinations, newest first, by project root.
#[derive(Debug, Default)]
pub(super) struct RecentDestinations {
    by_project: HashMap<PathBuf, Vec<String>>,
}

impl RecentDestinations {
    /// Put `dir` first in `project`'s list.
    fn record(&mut self, project: &Path, dir: String) {
        let list = self.by_project.entry(project.to_path_buf()).or_default();
        list.retain(|known| known != &dir);
        list.insert(0, dir);
        list.truncate(MAX_RECENT);
    }

    /// `project`'s destinations, newest first.
    fn of(&self, project: &Path) -> &[String] {
        self.by_project.get(project).map_or(&[], Vec::as_slice)
    }
}

/// The directory a confirmed path puts things in, ending in `/`.
///
/// `names_item`: the path names the item made (a single file copied, an
/// archive, a saved file), so its parent is the directory unless the path
/// ends in `/` or is an existing directory.
fn destination_dir(base: &Path, input: &str, names_item: bool) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if termide_vfs::is_vfs_url(input) {
        let dir = if names_item && !input.ends_with('/') {
            &input[..=input.rfind('/')?]
        } else {
            input
        };
        return Some(with_slash(dir.to_string()));
    }
    let (path, explicit_dir) = path_utils::resolve_local_destination_input(base, input);
    let path = lexically_normal(&path);
    let dir = if names_item && !explicit_dir && !path.is_dir() {
        path.parent()?.to_path_buf()
    } else {
        path
    };
    Some(with_slash(dir.display().to_string()))
}

/// `path` with `.` dropped and each `..` taking off the name before it.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

fn with_slash(dir: String) -> String {
    if dir.ends_with('/') {
        dir
    } else {
        format!("{dir}/")
    }
}

impl App {
    /// The quick destinations of a path prompt: the panels' directories,
    /// this project's recent destinations, then the drives (Windows) or
    /// mount points (Unix: the root, disks, removable media, network
    /// shares), each once.
    pub(super) fn destination_roots(&self) -> Vec<QuickDestination> {
        let panels = self
            .find_all_other_panel_paths()
            .into_iter()
            .map(|dir| QuickDestination::new(dir, DestinationKind::Panel));
        let recent = self
            .recent_destinations
            .of(&self.project_root)
            .iter()
            .map(|dir| QuickDestination::new(dir.clone(), DestinationKind::Recent));
        let mounts = termide_vfs::drive_roots()
            .into_iter()
            .chain(termide_system_monitor::browsable_mount_points())
            .map(|path| {
                QuickDestination::new(
                    with_slash(path.display().to_string()),
                    DestinationKind::Mount,
                )
            });
        let mut roots: Vec<QuickDestination> = Vec::new();
        for root in panels.chain(recent).chain(mounts) {
            if !roots.iter().any(|known| known.path == root.path) {
                roots.push(root);
            }
        }
        roots
    }

    /// Remember the directory a path prompt about to close was confirmed
    /// with.
    pub(super) fn remember_destination(&mut self, result: &ModalResult<Box<dyn std::any::Any>>) {
        let ModalResult::Confirmed(value) = result else {
            return;
        };
        let (Some(modal), Some(action)) = (&self.state.active_modal, &self.state.pending_action)
        else {
            return;
        };
        if let Some(dir) = confirmed_destination(modal, action, value.as_ref()) {
            self.recent_destinations.record(&self.project_root, dir);
        }
    }
}

/// The destination directory `value` confirms in `modal` for `action`.
/// Only the destination prompts count: a rename, which shares the move
/// action, asks in a plain input.
fn confirmed_destination(
    modal: &ActiveModal,
    action: &PendingAction,
    value: &dyn std::any::Any,
) -> Option<String> {
    match (modal, action) {
        (
            ActiveModal::EditableSelect(_),
            PendingAction::CopyPath {
                sources,
                target_directory,
                ..
            }
            | PendingAction::MovePath {
                sources,
                target_directory,
            },
        ) => {
            let input = value.downcast_ref::<String>()?;
            let base = target_directory
                .clone()
                .or_else(|| sources.first()?.parent().map(Path::to_path_buf))?;
            destination_dir(&base, input, sources.len() == 1)
        }
        (ActiveModal::EditableSelect(_), PendingAction::PackPaths { sources, .. }) => {
            let input = value.downcast_ref::<String>()?;
            destination_dir(sources.first()?.parent()?, input, true)
        }
        (ActiveModal::EditableSelect(_), PendingAction::UnpackArchives { archives }) => {
            unpack_destination_dir(archives, value.downcast_ref::<String>()?)
        }
        (
            ActiveModal::SaveAs(_),
            PendingAction::SaveFileAs { directory }
            | PendingAction::SaveContentAs { directory, .. },
        ) => destination_dir(directory, &value.downcast_ref::<SaveAsResult>()?.path, true),
        _ => None,
    }
}

/// The directory an unpack prompt was confirmed with. A single archive's
/// own directory (`dir/name/`, offered for it alone) counts as `dir/`.
fn unpack_destination_dir(archives: &[PathBuf], input: &str) -> Option<String> {
    let base = archives.first()?.parent()?;
    let dir = destination_dir(base, input, false)?;
    if let [archive] = archives {
        let own = format!("/{}/", termide_file_ops::unpack_dir_name(archive));
        if let Some(parent) = dir.strip_suffix(&own) {
            return Some(format!("{parent}/"));
        }
    }
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_prompt() -> ActiveModal {
        ActiveModal::EditableSelect(Box::new(termide_modal::EditableSelectModal::new(
            "t",
            "p",
            "",
            Vec::new(),
        )))
    }

    #[test]
    fn a_confirmed_copy_counts_and_a_rename_does_not() {
        let copy = PendingAction::CopyPath {
            sources: vec![PathBuf::from("/nowhere/src/a.txt")],
            target_directory: Some(PathBuf::from("/nowhere/src")),
            create_symlink: false,
            create_relative_symlink: false,
        };
        let typed: Box<dyn std::any::Any> = Box::new("/nowhere/out/a.txt".to_string());
        assert_eq!(
            confirmed_destination(&path_prompt(), &copy, typed.as_ref()).as_deref(),
            Some("/nowhere/out/")
        );

        let rename = PendingAction::MovePath {
            sources: vec![PathBuf::from("/nowhere/src/a.txt")],
            target_directory: Some(PathBuf::from("/nowhere/src")),
        };
        let input = ActiveModal::Input(Box::new(termide_modal::InputModal::new("t", "p")));
        let typed: Box<dyn std::any::Any> = Box::new("b.txt".to_string());
        assert_eq!(confirmed_destination(&input, &rename, typed.as_ref()), None);
    }

    #[test]
    fn a_saved_file_counts_by_its_directory() {
        let save = PendingAction::SaveFileAs {
            directory: PathBuf::from("/nowhere/doc"),
        };
        let modal = ActiveModal::SaveAs(Box::new(termide_modal::SaveAsModal::new("t", "")));
        let saved: Box<dyn std::any::Any> = Box::new(SaveAsResult {
            path: "notes/x.md".to_string(),
            executable: false,
        });
        assert_eq!(
            confirmed_destination(&modal, &save, saved.as_ref()).as_deref(),
            Some("/nowhere/doc/notes/")
        );
    }

    #[test]
    fn a_destination_moves_to_the_front_and_the_list_is_capped() {
        let mut recent = RecentDestinations::default();
        let project = Path::new("/p");
        for i in 0..MAX_RECENT + 2 {
            recent.record(project, format!("/d{i}/"));
        }
        recent.record(project, "/d5/".to_string());
        let list = recent.of(project);
        assert_eq!(list.len(), MAX_RECENT);
        assert_eq!(list[0], "/d5/");
        assert_eq!(list.iter().filter(|d| *d == "/d5/").count(), 1);
        assert!(!list.contains(&"/d0/".to_string()));
    }

    #[test]
    fn each_project_keeps_its_own_list() {
        let mut recent = RecentDestinations::default();
        recent.record(Path::new("/a"), "/x/".to_string());
        assert_eq!(recent.of(Path::new("/a")), ["/x/".to_string()]);
        assert!(recent.of(Path::new("/b")).is_empty());
    }

    #[test]
    fn a_path_naming_the_item_counts_by_its_directory() {
        let base = Path::new("/nowhere/base");
        assert_eq!(
            destination_dir(base, "/nowhere/out/file.txt", true).as_deref(),
            Some("/nowhere/out/")
        );
        assert_eq!(
            destination_dir(base, "/nowhere/out/", true).as_deref(),
            Some("/nowhere/out/")
        );
        assert_eq!(
            destination_dir(base, "/nowhere/out", false).as_deref(),
            Some("/nowhere/out/")
        );
        assert_eq!(
            destination_dir(base, "../sib/a.zip", true).as_deref(),
            Some("/nowhere/sib/")
        );
        assert_eq!(destination_dir(base, "  ", true), None);
    }

    #[test]
    fn an_existing_directory_counts_as_itself() {
        let dir = std::env::temp_dir();
        let expected = with_slash(dir.display().to_string());
        assert_eq!(
            destination_dir(Path::new("/"), &dir.display().to_string(), true),
            Some(expected)
        );
    }

    #[test]
    fn a_remote_path_counts_by_its_directory() {
        assert_eq!(
            destination_dir(Path::new("/"), "sftp://u@h/srv/f.txt", true).as_deref(),
            Some("sftp://u@h/srv/")
        );
        assert_eq!(
            destination_dir(Path::new("/"), "sftp://u@h/srv", false).as_deref(),
            Some("sftp://u@h/srv/")
        );
    }

    #[test]
    fn an_archive_own_directory_counts_as_its_parent() {
        let archives = [PathBuf::from("/src/data.tar.gz")];
        assert_eq!(
            unpack_destination_dir(&archives, "/out/data/").as_deref(),
            Some("/out/")
        );
        assert_eq!(
            unpack_destination_dir(&archives, "/out/other/").as_deref(),
            Some("/out/other/")
        );
        let many = [PathBuf::from("/src/a.zip"), PathBuf::from("/src/data.zip")];
        assert_eq!(
            unpack_destination_dir(&many, "/out/data/").as_deref(),
            Some("/out/data/")
        );
    }
}
