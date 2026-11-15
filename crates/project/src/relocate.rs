//! What termide keeps of a project, carried along when the project's
//! directory is moved or copied: its directory in the store — and those of
//! the projects inside it, since their files went along — with the paths in
//! the layouts following the files, and for a move what the user granted it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::ownership::{LOCK_FILE, OWNER_FILE};
use crate::store::{
    delete_dir, holds_only_owner, nested_in, project_dir_in, projects_dir_in, PROJECT_FILE,
    TRUST_FILE,
};
use crate::{get_data_dir, PanelState, ProjectFileOut, ProjectLayout};

/// Carry the stored state of the project that was at `from` to the one now
/// at `to`, and of the projects inside it: move it, or with `copy` copy it,
/// leaving the grants behind. The paths in the layouts follow the files; a
/// session log follows `sessions`, which maps the old paths of the logs
/// carried along to their new ones.
///
/// # Errors
///
/// When there is no data directory or the state cannot be moved or copied.
pub fn relocate_project_state(
    from: &Path,
    to: &Path,
    copy: bool,
    sessions: &HashMap<PathBuf, PathBuf>,
) -> Result<()> {
    relocate_state_in(&get_data_dir()?, from, to, copy, sessions)
}

/// Make the project at `from` the one at `to` without moving any file, as
/// changing a project's root does: its layout, buffers and logs go to `to`
/// as they are. What was granted stays with `from`: it was granted for what
/// `from` holds.
///
/// # Errors
///
/// When there is no data directory or the state cannot be moved.
pub fn rehome_project_state(from: &Path, to: &Path) -> Result<()> {
    let data_dir = get_data_dir()?;
    let (src, dst) = (
        project_dir_in(&data_dir, from),
        project_dir_in(&data_dir, to),
    );
    carry(&src, &dst, false, false)
        .with_context(|| format!("Failed to carry {} to {}", src.display(), dst.display()))?;
    rewrite_project_file(&dst.join(PROJECT_FILE), to, &|_| None)
}

fn relocate_state_in(
    data_dir: &Path,
    from: &Path,
    to: &Path,
    copy: bool,
    sessions: &HashMap<PathBuf, PathBuf>,
) -> Result<()> {
    let map = |path: &Path| {
        sessions
            .get(path)
            .cloned()
            .or_else(|| termide_fsutil::rebase(path, from, to))
    };
    let mut roots = vec![(from.to_path_buf(), to.to_path_buf())];
    roots.extend(
        nested_in(&projects_dir_in(data_dir), from)
            .into_iter()
            .filter_map(|nested| {
                let new = termide_fsutil::rebase(&nested.project_path, from, to)?;
                Some((nested.project_path, new))
            }),
    );
    for (old_root, new_root) in roots {
        let (src, dst) = (
            project_dir_in(data_dir, &old_root),
            project_dir_in(data_dir, &new_root),
        );
        // A copy is another project: what was granted to this one is not
        // granted to it.
        carry(&src, &dst, copy, !copy)
            .with_context(|| format!("Failed to carry {} to {}", src.display(), dst.display()))?;
        if let Err(e) = rewrite_project_file(&dst.join(PROJECT_FILE), &new_root, &map) {
            log::warn!("Could not rewrite the layout in {}: {e:#}", dst.display());
        }
    }
    Ok(())
}

/// Move or copy what the store directory `src` holds into `dst`, but its
/// lock and owner note, which stay with whoever holds them, and, unless
/// `grants`, its `trust.toml`. A `src` left with nothing but its lock goes
/// unless someone holds it: then its holder removes it when letting go.
fn carry(src: &Path, dst: &Path, copy: bool, grants: bool) -> std::io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == LOCK_FILE || name == OWNER_FILE || (!grants && name == TRUST_FILE) {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(&name));
        if entry.file_type()?.is_dir() {
            termide_fsutil::transfer_tree(&from, &to, copy, &|_| false)?;
        } else if copy {
            fs::copy(&from, &to)?;
        } else if fs::rename(&from, &to).is_err() {
            fs::copy(&from, &to)?;
            fs::remove_file(&from)?;
        }
    }
    if !copy && holds_only_owner(src) {
        let _ = delete_dir(src);
    }
    Ok(())
}

/// Name `root` in the `project.toml` at `path` and point the paths of its
/// layout where `map` says. A layout that does not parse is left as it is,
/// only the root changed.
fn rewrite_project_file(
    path: &Path,
    root: &Path,
    map: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut table: toml::Table = toml::from_str(&text)?;
    let layout = table
        .get("layout")
        .cloned()
        .and_then(|value| value.try_into::<ProjectLayout>().ok());
    let text = match layout {
        Some(mut layout) => {
            for panel in layout
                .panel_groups
                .iter_mut()
                .flat_map(|group| group.panels.iter_mut())
            {
                map_panel_paths(panel, map);
            }
            toml::to_string_pretty(&ProjectFileOut {
                root,
                layout: &layout,
            })?
        }
        None => {
            table.insert(
                "root".into(),
                toml::Value::String(root.to_string_lossy().into_owned()),
            );
            toml::to_string_pretty(&table)?
        }
    };
    termide_fsutil::write_atomic(path, text.as_bytes())?;
    Ok(())
}

/// Replace each path of `panel` that `map` relocates.
fn map_panel_paths(panel: &mut PanelState, map: &dyn Fn(&Path) -> Option<PathBuf>) {
    let apply = |path: &mut PathBuf| {
        if let Some(new) = map(path) {
            *path = new;
        }
    };
    match panel {
        // A remote URL names no local file.
        PanelState::FileManager { path_or_url } => {
            if !path_or_url.contains("://") {
                if let Some(new) = map(Path::new(path_or_url.as_str())) {
                    *path_or_url = new.to_string_lossy().into_owned();
                }
            }
        }
        PanelState::Editor { path, .. } => {
            if let Some(path) = path {
                apply(path);
            }
        }
        PanelState::Terminal { working_dir } => apply(working_dir),
        PanelState::Image { path }
        | PanelState::Binary { path }
        | PanelState::Markdown { path }
        | PanelState::Mermaid { path }
        | PanelState::Csv { path }
        | PanelState::Html { path } => apply(path),
        PanelState::GitStatus { repo_path }
        | PanelState::GitLog { repo_path }
        | PanelState::GitDiff { repo_path, .. } => apply(repo_path),
        PanelState::Agent { cwd, session, .. } => {
            apply(cwd);
            if let Some(session) = session {
                apply(session);
            }
        }
        PanelState::Journal
        | PanelState::Outline
        | PanelState::Diagnostics
        | PanelState::Database { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ownership::claim_in;
    use crate::store::BUFFERS_DIR;
    use crate::PanelGroupState;

    fn group(panels: Vec<PanelState>) -> PanelGroupState {
        PanelGroupState {
            panels,
            expanded_index: 0,
            width: None,
            mode: Default::default(),
            split_heights: None,
            fullscreen_cache: None,
        }
    }

    fn save(data: &Path, root: &Path, panels: Vec<PanelState>) -> PathBuf {
        let dir = project_dir_in(data, root);
        fs::create_dir_all(&dir).unwrap();
        let layout = ProjectLayout {
            panel_groups: vec![group(panels)],
            focused_group: 0,
        };
        let text = toml::to_string_pretty(&ProjectFileOut {
            root,
            layout: &layout,
        })
        .unwrap();
        fs::write(dir.join(PROJECT_FILE), text).unwrap();
        dir
    }

    fn read(data: &Path, root: &Path) -> (PathBuf, Vec<PanelState>) {
        let dir = project_dir_in(data, root);
        let text = fs::read_to_string(dir.join(PROJECT_FILE)).unwrap();
        let table: toml::Table = toml::from_str(&text).unwrap();
        let layout: ProjectLayout = table["layout"].clone().try_into().unwrap();
        (
            PathBuf::from(table["root"].as_str().unwrap()),
            layout.panel_groups.into_iter().next().unwrap().panels,
        )
    }

    /// The layout of the project and those of projects inside it move to
    /// their new directories with their roots, paths and session logs
    /// rewritten; paths outside the project and remote ones stay; the grants
    /// and buffers follow a move, the lock stays with its holder, and the
    /// old directories go.
    #[test]
    fn a_move_carries_layouts_and_grants() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let (from, to) = (
            Path::new("/nonexistent/a/proj"),
            Path::new("/nonexistent/b/new"),
        );
        let old_log = PathBuf::from("/cfg/ai/sessions/nonexistent/a/proj/x_1.jsonl");
        let new_log = PathBuf::from("/cfg/ai/sessions/nonexistent/b/new/x_1.jsonl");
        let old_dir = save(
            data,
            from,
            vec![
                PanelState::FileManager {
                    path_or_url: "/nonexistent/a/proj/src".into(),
                },
                PanelState::FileManager {
                    path_or_url: "sftp://host/nonexistent/a/proj".into(),
                },
                PanelState::Editor {
                    path: Some("/nonexistent/elsewhere/notes.md".into()),
                    unsaved_buffer_file: None,
                },
                PanelState::Agent {
                    cwd: from.to_path_buf(),
                    session: Some(old_log.clone()),
                    agent: None,
                    setup: Default::default(),
                },
            ],
        );
        fs::create_dir_all(old_dir.join(BUFFERS_DIR)).unwrap();
        fs::write(old_dir.join(BUFFERS_DIR).join("1.txt"), "draft").unwrap();
        fs::write(old_dir.join(TRUST_FILE), "[approved]\n").unwrap();
        let inner_dir = save(
            data,
            &from.join("inner"),
            vec![PanelState::Terminal {
                working_dir: from.join("inner"),
            }],
        );
        // The project held through its move, its new place too.
        let from_claim = claim_in(&old_dir, None).unwrap();
        let new_dir = project_dir_in(data, to);
        let to_claim = claim_in(&new_dir, None).unwrap();

        let sessions = HashMap::from([(old_log, new_log.clone())]);
        relocate_state_in(data, from, to, false, &sessions).unwrap();

        let (root, panels) = read(data, to);
        assert_eq!(root, to);
        assert_eq!(
            panels,
            [
                PanelState::FileManager {
                    path_or_url: "/nonexistent/b/new/src".into(),
                },
                PanelState::FileManager {
                    path_or_url: "sftp://host/nonexistent/a/proj".into(),
                },
                PanelState::Editor {
                    path: Some("/nonexistent/elsewhere/notes.md".into()),
                    unsaved_buffer_file: None,
                },
                PanelState::Agent {
                    cwd: to.to_path_buf(),
                    session: Some(new_log),
                    agent: None,
                    setup: Default::default(),
                },
            ]
        );
        assert_eq!(
            read(data, &to.join("inner")),
            (
                to.join("inner"),
                vec![PanelState::Terminal {
                    working_dir: to.join("inner"),
                }]
            )
        );
        assert!(new_dir.join(BUFFERS_DIR).join("1.txt").exists());
        assert!(new_dir.join(TRUST_FILE).exists());
        assert!(!inner_dir.exists());
        // Held still: only the lock is left, and it goes when let go.
        assert!(holds_only_owner(&old_dir));
        drop(from_claim);
        assert!(!old_dir.exists());
        drop(to_claim);
        assert!(new_dir.join(PROJECT_FILE).exists());
    }

    /// A copy leaves the original whole and grants the copy nothing.
    #[test]
    fn a_copy_leaves_the_original_and_its_grants() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let (from, to) = (
            Path::new("/nonexistent/a/proj"),
            Path::new("/nonexistent/a/copy"),
        );
        let terminal = |root: &Path| PanelState::Terminal {
            working_dir: root.to_path_buf(),
        };
        let old_dir = save(data, from, vec![terminal(from)]);
        fs::write(old_dir.join(TRUST_FILE), "").unwrap();

        relocate_state_in(data, from, to, true, &HashMap::new()).unwrap();

        assert_eq!(read(data, from), (from.to_path_buf(), vec![terminal(from)]));
        assert_eq!(read(data, to), (to.to_path_buf(), vec![terminal(to)]));
        assert!(old_dir.join(TRUST_FILE).exists());
        assert!(!project_dir_in(data, to).join(TRUST_FILE).exists());
    }
}
