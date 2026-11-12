//! What termide keeps of a project, carried along when the project's
//! directory is moved or copied: the saved layouts filed under its path —
//! those of projects inside it with them, since their files went along —
//! and, for a move, what the user granted it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::{get_data_dir, project_key, PanelState, ProjectLayout, PROJECTS_DIR, TRUST_DIR};

/// The name of a layout file in a project's storage directory.
const LAYOUT_FILE: &str = "session.toml";

/// Carry the stored state of the project that was at `from` to the one now
/// at `to`: move it, or with `copy` copy it, leaving the grants behind. The
/// paths in the layouts follow the files; a session log follows `sessions`,
/// which maps the old paths of the logs carried along to their new ones.
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

fn relocate_state_in(
    data_dir: &Path,
    from: &Path,
    to: &Path,
    copy: bool,
    sessions: &HashMap<PathBuf, PathBuf>,
) -> Result<()> {
    let (from_key, to_key) = (project_key(from), project_key(to));
    let projects = data_dir.join(PROJECTS_DIR);
    let placed = transfer(&projects, &from_key, &to_key, copy)?;
    let map = |path: &Path| {
        sessions
            .get(path)
            .cloned()
            .or_else(|| termide_fsutil::rebase(path, from, to))
    };
    for layout_file in placed.iter().filter(|p| p.ends_with(LAYOUT_FILE)) {
        if let Err(e) = rewrite_layout(layout_file, &map) {
            log::warn!("Could not rewrite {}: {e:#}", layout_file.display());
        }
    }
    // A copy is another project: what was granted to this one is not
    // granted to it.
    if !copy {
        transfer(&data_dir.join(TRUST_DIR), &from_key, &to_key, false)?;
    }
    Ok(())
}

/// Move or copy `base/from_key` to `base/to_key`, dropping the directories
/// a move leaves empty; the files placed.
fn transfer(base: &Path, from_key: &Path, to_key: &Path, copy: bool) -> Result<Vec<PathBuf>> {
    let (src, dst) = (base.join(from_key), base.join(to_key));
    let placed = termide_fsutil::transfer_tree(&src, &dst, copy, &|_| false)
        .with_context(|| format!("Failed to carry {} to {}", src.display(), dst.display()))?;
    if !copy {
        if let Some(parent) = src.parent() {
            termide_fsutil::prune_empty_dirs(parent, base);
        }
    }
    Ok(placed)
}

/// Point the paths of the layout file at `path` where `map` says.
fn rewrite_layout(path: &Path, map: &dyn Fn(&Path) -> Option<PathBuf>) -> Result<()> {
    let text = std::fs::read_to_string(path)?;
    let mut layout: ProjectLayout = toml::from_str(&text)?;
    for panel in layout
        .panel_groups
        .iter_mut()
        .flat_map(|group| group.panels.iter_mut())
    {
        map_panel_paths(panel, map);
    }
    let text = toml::to_string_pretty(&layout)?;
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

    fn save(dir: &Path, panels: Vec<PanelState>) {
        std::fs::create_dir_all(dir).unwrap();
        let layout = ProjectLayout {
            panel_groups: vec![group(panels)],
            focused_group: 0,
        };
        std::fs::write(
            dir.join(LAYOUT_FILE),
            toml::to_string_pretty(&layout).unwrap(),
        )
        .unwrap();
    }

    fn panels(dir: &Path) -> Vec<PanelState> {
        let text = std::fs::read_to_string(dir.join(LAYOUT_FILE)).unwrap();
        let layout: ProjectLayout = toml::from_str(&text).unwrap();
        layout.panel_groups.into_iter().next().unwrap().panels
    }

    /// The layout of the project and those of projects inside it move to
    /// the new key with their paths and session logs rewritten; paths
    /// outside the project and remote ones stay; the grants follow a move,
    /// and the old directories go.
    #[test]
    fn a_move_carries_layouts_and_grants() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let (from, to) = (
            Path::new("/nonexistent/a/proj"),
            Path::new("/nonexistent/b/new"),
        );
        let projects = data.join(PROJECTS_DIR);
        let old_dir = projects.join(project_key(from));
        let old_log = PathBuf::from("/cfg/ai/sessions/nonexistent/a/proj/x_1.jsonl");
        let new_log = PathBuf::from("/cfg/ai/sessions/nonexistent/b/new/x_1.jsonl");
        save(
            &old_dir,
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
        std::fs::write(old_dir.join("unsaved-1.txt"), "draft").unwrap();
        save(
            &old_dir.join("inner"),
            vec![PanelState::Terminal {
                working_dir: from.join("inner"),
            }],
        );
        let trust = data.join(TRUST_DIR).join(project_key(from));
        std::fs::create_dir_all(&trust).unwrap();
        std::fs::write(trust.join("permissions.toml"), "allow = []").unwrap();

        let sessions = HashMap::from([(old_log, new_log.clone())]);
        relocate_state_in(data, from, to, false, &sessions).unwrap();

        let new_dir = projects.join(project_key(to));
        assert_eq!(
            panels(&new_dir),
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
            panels(&new_dir.join("inner")),
            [PanelState::Terminal {
                working_dir: to.join("inner"),
            }]
        );
        assert!(new_dir.join("unsaved-1.txt").exists());
        assert!(!projects.join("nonexistent/a").exists());
        assert!(data
            .join(TRUST_DIR)
            .join(project_key(to))
            .join("permissions.toml")
            .exists());
        assert!(!data.join(TRUST_DIR).join("nonexistent/a").exists());
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
        let projects = data.join(PROJECTS_DIR);
        let terminal = |root: &Path| PanelState::Terminal {
            working_dir: root.to_path_buf(),
        };
        save(&projects.join(project_key(from)), vec![terminal(from)]);
        let trust = data.join(TRUST_DIR).join(project_key(from));
        std::fs::create_dir_all(&trust).unwrap();
        std::fs::write(trust.join("approved.toml"), "").unwrap();

        relocate_state_in(data, from, to, true, &HashMap::new()).unwrap();

        assert_eq!(panels(&projects.join(project_key(from))), [terminal(from)]);
        assert_eq!(panels(&projects.join(project_key(to))), [terminal(to)]);
        assert!(trust.join("approved.toml").exists());
        assert!(!data.join(TRUST_DIR).join(project_key(to)).exists());
    }
}
