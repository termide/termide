//! Saved project layouts for termide.
//!
//! Saves and restores application state between runs.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

mod maintenance;
pub use maintenance::*;
mod open_list;
pub use open_list::{load_open_projects, save_open_projects, SavedOpenProjects};

/// The saved layout of a project: its panels, restored on the next start
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectLayout {
    /// Panel groups (vertical columns with accordion)
    pub panel_groups: Vec<PanelGroupState>,
    /// Which group is currently focused (0-based index)
    pub focused_group: usize,
}

/// Legacy layout-mode tag retained for backward compatibility with
/// layouts saved before the unified-split refactor. Newer code never
/// writes this field; older layouts deserialize the tag and the
/// loader treats `Accordion` as a request to apply the
/// fullscreen-current-panel preset on top of `expanded_index`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupLayoutMode {
    #[default]
    #[serde(rename = "accordion")]
    Accordion,
    #[serde(rename = "split")]
    Split,
}

/// A group of panels (one vertical column).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelGroupState {
    /// Panels in this group.
    pub panels: Vec<PanelState>,
    /// Which panel is focused (0-based index).
    pub expanded_index: usize,
    /// Column width in characters (None = auto-distributed).
    pub width: Option<u16>,
    /// Legacy mode tag — still parsed from old layouts to drive
    /// fullscreen-preset migration. New layouts never write it.
    #[serde(default, skip_serializing)]
    pub mode: GroupLayoutMode,
    /// Cached panel heights (in lines). `None` means "no cache yet —
    /// derive equal distribution on first use".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_heights: Option<Vec<u16>>,
    /// When `Some`, the group is in the fullscreen-current-panel preset
    /// and this is the heights snapshot to restore on toggle-off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullscreen_cache: Option<Vec<u16>>,
}

/// Panel data for serialization
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PanelState {
    /// File manager panel
    #[serde(rename = "file_manager")]
    FileManager {
        /// Path for local filesystem, or VFS URL for remote (e.g., "sftp://user@host/path")
        #[serde(alias = "path")] // Support old format for backward compatibility
        path_or_url: String,
    },
    /// Text editor panel
    #[serde(rename = "editor")]
    Editor {
        /// File path (None for unnamed/scratch buffers)
        path: Option<PathBuf>,
        /// Temporary file name for unsaved buffers (format: unsaved-YYYYMMDD-HHIISS-MSEC.txt)
        #[serde(skip_serializing_if = "Option::is_none")]
        unsaved_buffer_file: Option<String>,
    },
    /// Terminal panel
    #[serde(rename = "terminal")]
    Terminal {
        /// Working directory
        working_dir: PathBuf,
    },
    /// Journal panel
    #[serde(rename = "journal")]
    Journal,
    /// Image viewer panel
    #[serde(rename = "image")]
    Image {
        /// Path to image file
        path: PathBuf,
    },
    /// Binary hex/ASCII viewer panel
    #[serde(rename = "binary")]
    Binary {
        /// Path to the binary file
        path: PathBuf,
    },
    /// Rendered Markdown preview panel
    #[serde(rename = "markdown")]
    Markdown {
        /// Path to the markdown file
        path: PathBuf,
    },
    /// Mermaid diagram viewer panel
    #[serde(rename = "mermaid")]
    Mermaid {
        /// Path to the `.mmd` file
        path: PathBuf,
    },
    /// Rendered HTML viewer panel
    #[serde(rename = "html")]
    Html {
        /// Path to the HTML file
        path: PathBuf,
    },
    /// Git status panel
    #[serde(rename = "git_status")]
    GitStatus {
        /// Repository path
        repo_path: PathBuf,
    },
    /// Git log panel
    #[serde(rename = "git_log")]
    GitLog {
        /// Repository path
        repo_path: PathBuf,
    },
    /// Git diff panel
    #[serde(rename = "git_diff")]
    GitDiff {
        /// Repository path
        repo_path: PathBuf,
        /// Commit hash (None = working directory changes, Some = specific commit)
        #[serde(skip_serializing_if = "Option::is_none")]
        commit_hash: Option<String>,
    },
    /// Outline panel (symbol navigator)
    #[serde(rename = "outline")]
    Outline,
    /// Diagnostics panel
    #[serde(rename = "diagnostics")]
    Diagnostics,
    /// Database viewer panel
    #[serde(rename = "database")]
    Database {
        /// Connection URL (as entered in the bookmark)
        url: String,
        /// Display label
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label: String,
    },
    /// Coding agent panel
    #[serde(rename = "agent")]
    Agent {
        /// Working directory the agent's tools run in
        cwd: PathBuf,
        /// Agent session log to continue; absent when the panel ran without one
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<PathBuf>,
        /// Agent definition in use; the default one when absent
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        /// What was set up on a session nothing was sent in yet. Its log is
        /// deleted as the panel closes, so a restore starts the fresh
        /// session on these instead
        #[serde(default, skip_serializing_if = "AgentSetupState::is_empty")]
        setup: AgentSetupState,
    },
    // Note: Welcome panels are NOT saved (they auto-close)
}

/// The settings picked on an agent panel before its first request: the
/// connection, the model, the reasoning level and the external agent's own
/// options, each absent when left as configured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSetupState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// The external agent's options as `[option, value]` pairs, in the
    /// order they were picked
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<(String, String)>,
}

impl AgentSetupState {
    /// Whether nothing was picked, so there is nothing to save.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The relative path a project's data is filed under: its canonical path
/// with the root components stripped (leading `/` on Unix; drive prefix and
/// `\` on Windows), so joining it onto a base directory nests rather than
/// replaces. `Component::Prefix` covers `C:` and `\\server\share`,
/// `Component::RootDir` covers `/` and `\`.
#[must_use]
pub fn project_key(project_root: &Path) -> PathBuf {
    let canonical =
        dunce::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    canonical
        .components()
        .filter(|c| {
            !matches!(
                c,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        })
        .collect()
}

/// Get the data directory for termide.
pub(crate) fn get_data_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|p| p.join("termide"))
        .context("Failed to determine data directory")
}

/// Directory holding every project's saved layout.
///
/// Before the rename this was `sessions`; [`migrate_legacy_layouts`] moves
/// the old tree across at startup so saved layouts survive an upgrade.
pub(crate) const PROJECTS_DIR: &str = "projects";
const LEGACY_PROJECTS_DIR: &str = "sessions";

/// Move the saved layouts of a termide older than the rename into place.
///
/// Runs once at startup, before the logger is up, so the outcome is returned
/// for the caller to log rather than logged here. Run on every launch: an
/// older termide may have written `sessions/` again since the last one.
pub fn migrate_legacy_layouts() -> Vec<(log::Level, String)> {
    match get_data_dir() {
        Ok(data_dir) => migrate_legacy_dir(&data_dir),
        Err(_) => Vec::new(),
    }
}

/// Move `<data>/sessions` to `<data>/projects`, or fold it into `projects`
/// when both exist. Failures are reported, never fatal: a fresh directory is
/// created instead and only the old layouts are lost.
fn migrate_legacy_dir(data_dir: &Path) -> Vec<(log::Level, String)> {
    let legacy = data_dir.join(LEGACY_PROJECTS_DIR);
    let current = data_dir.join(PROJECTS_DIR);
    if !legacy.is_dir() {
        return Vec::new();
    }
    let (from, to) = (legacy.display(), current.display());
    if !current.exists() {
        return vec![match std::fs::rename(&legacy, &current) {
            Ok(()) => (
                log::Level::Info,
                format!("moved saved layouts from {from} to {to}"),
            ),
            Err(e) => (
                log::Level::Warn,
                format!("could not move {from} to {to}: {e}"),
            ),
        }];
    }
    // Both exist: an older termide kept writing to the legacy directory after
    // the move. Fold what it left into the current one and drop the shell.
    vec![match merge_move(&legacy, &current) {
        Ok(()) => (
            log::Level::Info,
            format!("folded saved layouts from {from} into {to}"),
        ),
        Err(e) => (
            log::Level::Warn,
            format!("could not fold {from} into {to}: {e}"),
        ),
    }]
}

/// Move everything under `src` into `dst`, recursing into directories that
/// exist on both sides and keeping the newer of two files with the same
/// name. Directories emptied on the way are removed, `src` included.
fn merge_move(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if !to.exists() {
            std::fs::rename(&from, &to)?;
        } else if from.is_dir() && to.is_dir() {
            merge_move(&from, &to)?;
        } else if from.is_file() && to.is_file() {
            let newer = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            if newer(&from) > newer(&to) {
                std::fs::rename(&from, &to)?;
            } else {
                std::fs::remove_file(&from)?;
            }
        }
    }
    // Only an empty directory goes; anything unexpected stays for the user.
    let _ = std::fs::remove_dir(src);
    Ok(())
}

/// Remove the files of the project stored in `project_dir`, then the
/// directories left empty up to `projects_dir`.
///
/// Storage directories nest like the project paths they mirror, so the
/// subdirectories of `project_dir` hold the state of projects inside this
/// one and are kept.
fn delete_project_files(project_dir: &Path, projects_dir: &Path) -> std::io::Result<()> {
    if !project_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(project_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            fs::remove_file(entry.path())?;
        }
    }
    let mut dir = project_dir;
    while dir != projects_dir && dir.starts_with(projects_dir) && fs::remove_dir(dir).is_ok() {
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    Ok(())
}

impl ProjectLayout {
    /// Get the storage directory for a specific project
    ///
    /// Creates nested subdirectories matching the project path with root stripped.
    /// Example (Unix):    /home/user/project1 -> ~/.local/share/termide/projects/home/user/project1/
    /// Example (Windows): C:\Users\user\proj  -> %APPDATA%\termide\projects\Users\user\proj\
    pub fn get_project_dir(project_root: &Path) -> Result<PathBuf> {
        Ok(get_data_dir()?
            .join(PROJECTS_DIR)
            .join(project_key(project_root)))
    }

    /// Get the path to the layout file (`session.toml`) of a specific project
    pub fn get_project_path(project_root: &Path) -> Result<PathBuf> {
        Ok(Self::get_project_dir(project_root)?.join("session.toml"))
    }

    /// Delete the stored state of a specific project.
    pub fn delete_layout(project_root: &Path) -> Result<()> {
        let project_dir = Self::get_project_dir(project_root)?;
        let projects_dir = get_data_dir()?.join(PROJECTS_DIR);
        delete_project_files(&project_dir, &projects_dir)
            .with_context(|| format!("Failed to delete project layout: {}", project_dir.display()))
    }

    /// Load the saved layout of a specific project
    pub fn load(project_root: &Path) -> Result<Self> {
        let path = Self::get_project_path(project_root)?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read layout file: {}", path.display()))?;
        let layout: ProjectLayout = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse layout file: {}", path.display()))?;
        Ok(layout)
    }

    /// Save this layout for a specific project
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let project_dir = Self::get_project_dir(project_root)?;

        // Ensure the project directory exists
        fs::create_dir_all(&project_dir).with_context(|| {
            format!(
                "Failed to create project directory: {}",
                project_dir.display()
            )
        })?;

        let path = project_dir.join("session.toml");
        let contents = toml::to_string_pretty(self).context("Failed to serialize layout")?;

        fs::write(&path, contents)
            .with_context(|| format!("Failed to write layout file: {}", path.display()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Panels {
        panels: Vec<PanelState>,
    }

    /// Deleting a project keeps the projects stored inside it and prunes
    /// only the directories it leaves empty.
    #[test]
    fn deleting_a_project_keeps_nested_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join(PROJECTS_DIR);
        let outer = projects.join("home/u/notes");
        let inner = outer.join("journal");
        let lone = projects.join("home/u/work/api");
        for dir in [&inner, &lone] {
            fs::create_dir_all(dir).unwrap();
        }
        for file in [
            outer.join("session.toml"),
            outer.join("session-1.log"),
            inner.join("session.toml"),
            lone.join("session.toml"),
        ] {
            fs::write(file, "").unwrap();
        }

        delete_project_files(&outer, &projects).unwrap();
        assert!(!outer.join("session.toml").exists());
        assert!(!outer.join("session-1.log").exists());
        assert!(inner.join("session.toml").exists());

        delete_project_files(&lone, &projects).unwrap();
        assert!(!projects.join("home/u/work").exists());
        assert!(projects.join("home/u").exists());

        delete_project_files(&inner, &projects).unwrap();
        assert!(!projects.join("home").exists());
        assert!(projects.exists());
    }

    /// The agent variant serialises like the others and an absent session
    /// log is left out rather than written as an empty value.
    #[test]
    fn agent_panel_state_round_trips_through_toml() {
        let panels = Panels {
            panels: vec![
                PanelState::Agent {
                    cwd: PathBuf::from("/work"),
                    session: Some(PathBuf::from("/data/agent/s.jsonl")),
                    agent: Some("review".into()),
                    setup: AgentSetupState::default(),
                },
                PanelState::Agent {
                    cwd: PathBuf::from("/work"),
                    session: None,
                    agent: None,
                    setup: AgentSetupState {
                        connection: Some("local".into()),
                        model: Some("qwen".into()),
                        thinking: Some("high".into()),
                        options: vec![("model".into(), "opus".into())],
                    },
                },
            ],
        };
        let text = toml::to_string(&panels).unwrap();
        assert!(text.contains("type = \"agent\""), "{text}");
        assert_eq!(text.matches("session").count(), 1, "{text}");
        assert_eq!(text.matches("[panels.setup]").count(), 1, "{text}");
        let back: Panels = toml::from_str(&text).unwrap();
        assert_eq!(back.panels, panels.panels);
    }
    /// A termide older than the rename keeps writing `sessions/` after the
    /// first move; what it leaves there is folded into `projects/`, newer
    /// files winning, and the empty shell disappears.
    #[test]
    fn leftover_legacy_layouts_are_folded_into_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join(LEGACY_PROJECTS_DIR);
        let current = tmp.path().join(PROJECTS_DIR);
        let write = |base: &Path, rel: &str, body: &str| {
            let path = base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        write(&legacy, "home/u/only-legacy/session.toml", "legacy only");
        write(&legacy, "home/u/both/session.toml", "legacy newer");
        write(&current, "home/u/both/session.toml", "current older");
        write(&current, "home/u/only-current/session.toml", "current only");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        std::fs::File::open(current.join("home/u/both/session.toml"))
            .unwrap()
            .set_modified(old)
            .unwrap();

        let notes = migrate_legacy_dir(tmp.path());

        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, log::Level::Info, "{}", notes[0].1);
        assert!(!legacy.exists());
        let read = |rel: &str| std::fs::read_to_string(current.join(rel)).unwrap();
        assert_eq!(read("home/u/only-legacy/session.toml"), "legacy only");
        assert_eq!(read("home/u/both/session.toml"), "legacy newer");
        assert_eq!(read("home/u/only-current/session.toml"), "current only");

        // Idempotent: nothing left to do.
        assert!(migrate_legacy_dir(tmp.path()).is_empty());
        assert!(current.join("home/u/both/session.toml").is_file());
    }
    /// The first launch after the upgrade finds only `sessions/` and moves
    /// the whole tree, reporting it for the journal.
    #[test]
    fn legacy_layouts_move_to_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join(LEGACY_PROJECTS_DIR);
        let current = tmp.path().join(PROJECTS_DIR);
        std::fs::create_dir_all(legacy.join("home/u/proj")).unwrap();
        std::fs::write(legacy.join("home/u/proj/session.toml"), "layout").unwrap();

        let notes = migrate_legacy_dir(tmp.path());

        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, log::Level::Info, "{}", notes[0].1);
        assert!(!legacy.exists());
        let moved = std::fs::read_to_string(current.join("home/u/proj/session.toml")).unwrap();
        assert_eq!(moved, "layout");
        assert!(migrate_legacy_dir(tmp.path()).is_empty());
    }
    #[test]
    fn project_key_mirrors_the_path_without_its_root() {
        assert_eq!(
            project_key(Path::new("/nonexistent/home/u/proj")),
            PathBuf::from("nonexistent/home/u/proj")
        );
    }
}
