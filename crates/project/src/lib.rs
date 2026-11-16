//! Saved project layouts for termide.
//!
//! Saves and restores application state between runs.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

mod maintenance;
pub use maintenance::*;
mod migrate;
pub use migrate::migrate_legacy_layouts;
mod open_list;
pub use open_list::{load_open_projects, save_open_projects, SavedOpenProjects};
mod ownership;
mod relocate;
mod store;
pub use ownership::{claim_project, project_owner, Owner, ProjectClaim};
pub use relocate::{relocate_project_state, relocate_session_paths};
use store::PROJECT_FILE;
pub use store::{
    delete_project, list_all_projects, project_dir, project_id, sweep_store, trust_path,
    DeleteError, ProjectInfo,
};

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
        /// File name of the unsaved changes in the project's `buffers/`
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
    /// Table viewer for a delimited text file (CSV, TSV)
    #[serde(rename = "csv")]
    Csv {
        /// Path to the delimited file
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

/// The relative path a directory's agent session logs are filed under (see
/// `termide_agent_core`): its canonical path with the root components
/// stripped (leading `/` on Unix; drive prefix and `\` on Windows), so
/// joining it onto a base directory nests rather than replaces.
/// `Component::Prefix` covers `C:` and `\\server\share`,
/// `Component::RootDir` covers `/` and `\`.
///
/// The project store does not use it: projects are kept side by side, see
/// [`project_id`].
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

/// The file holding the "allow always" permission rules given for the
/// project at `project_root`, in the shape of `[ai.permissions]`: its
/// [`trust_path`].
///
/// # Errors
///
/// When there is no data directory.
pub fn granted_rules_path(project_root: &Path) -> Result<PathBuf> {
    trust_path(project_root)
}

/// The file holding the launches approved for the project at
/// `project_root`: the MCP servers and hooks it brings (see
/// `termide_agent_core::approvals`), under `[approved]` in its
/// [`trust_path`].
///
/// # Errors
///
/// When there is no data directory.
pub fn approvals_path(project_root: &Path) -> Result<PathBuf> {
    trust_path(project_root)
}

/// Get the data directory for termide.
pub(crate) fn get_data_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|p| p.join("termide"))
        .context("Failed to determine data directory")
}

/// What `project.toml` holds as it is written: the root, then the layout.
#[derive(Serialize)]
struct ProjectFileOut<'a> {
    root: &'a Path,
    layout: &'a ProjectLayout,
}

/// What `project.toml` holds as it is read. The root is the store's to
/// know; a file without a layout is a project with none saved.
#[derive(Deserialize)]
struct ProjectFileIn {
    #[serde(default)]
    layout: Option<ProjectLayout>,
}

impl ProjectLayout {
    /// The directory the project at `project_root` is kept in, see
    /// [`project_dir`].
    pub fn get_project_dir(project_root: &Path) -> Result<PathBuf> {
        project_dir(project_root)
    }

    /// The file holding the project's root and layout (`project.toml`).
    pub fn get_project_path(project_root: &Path) -> Result<PathBuf> {
        Ok(project_dir(project_root)?.join(PROJECT_FILE))
    }

    /// Load the saved layout of a specific project
    pub fn load(project_root: &Path) -> Result<Self> {
        let path = Self::get_project_path(project_root)?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read layout file: {}", path.display()))?;
        let file: ProjectFileIn = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse layout file: {}", path.display()))?;
        file.layout
            .with_context(|| format!("No layout saved in {}", path.display()))
    }

    /// Load the saved layout of a project to restore it, as [`Self::load`]
    /// does, but move a file that cannot be parsed — damaged, or written by
    /// a newer termide — aside first, so that the default layout the caller
    /// falls back to does not overwrite it at the next save. The error names
    /// where the file went.
    pub fn load_for_restore(project_root: &Path) -> Result<Self> {
        load_or_set_aside(&Self::get_project_path(project_root)?)
    }

    /// Save this layout for a specific project
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let project_dir = Self::get_project_dir(project_root)?;
        fs::create_dir_all(&project_dir).with_context(|| {
            format!(
                "Failed to create project directory: {}",
                project_dir.display()
            )
        })?;
        let path = project_dir.join(PROJECT_FILE);
        let contents = toml::to_string_pretty(&ProjectFileOut {
            root: project_root,
            layout: self,
        })
        .context("Failed to serialize layout")?;
        termide_fsutil::write_atomic(&path, contents.as_bytes())
            .with_context(|| format!("Failed to write layout file: {}", path.display()))?;
        Ok(())
    }
}

/// Parse the layout in the `project.toml` at `path`; when the file is there
/// but unreadable, rename it to `project.toml.broken-<local time>` beside
/// itself.
fn load_or_set_aside(path: &Path) -> Result<ProjectLayout> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read layout file: {}", path.display()))?;
    let error = match toml::from_str::<ProjectFileIn>(&contents) {
        Ok(file) => {
            return file
                .layout
                .with_context(|| format!("No layout saved in {}", path.display()))
        }
        Err(e) => e,
    };
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".broken-{stamp}"));
    let aside = PathBuf::from(aside);
    match fs::rename(path, &aside) {
        Ok(()) => Err(anyhow::anyhow!(
            "Failed to parse layout file {} ({}); it was kept as {}",
            path.display(),
            error.message().trim(),
            aside.display()
        )),
        Err(e) => Err(anyhow::anyhow!(
            "Failed to parse layout file {} ({}), and could not move it aside: {e}",
            path.display(),
            error.message().trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layout that does not parse is moved aside, not left to be
    /// overwritten by the default layout's first save; a readable one, one
    /// with no layout and a missing one are left alone.
    #[test]
    fn an_unreadable_layout_is_moved_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PROJECT_FILE);
        let names = || {
            let mut names: Vec<String> = fs::read_dir(tmp.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };

        assert!(load_or_set_aside(&path).is_err());
        assert!(names().is_empty());

        fs::write(&path, "root = \"/p\"\n").unwrap();
        assert!(load_or_set_aside(&path).is_err());
        assert_eq!(names(), [PROJECT_FILE]);

        fs::write(
            &path,
            "root = \"/p\"\n[layout]\nfocused_group = 0\npanel_groups = []\n",
        )
        .unwrap();
        assert!(load_or_set_aside(&path).is_ok());
        assert_eq!(names(), [PROJECT_FILE]);

        let broken = "root = \"/p\"\n[layout]\nfocused_group = 0\n[[layout.panel_groups]\n";
        fs::write(&path, broken).unwrap();
        let error = load_or_set_aside(&path).unwrap_err().to_string();
        let left = names();
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].starts_with("project.toml.broken-"), "{left:?}");
        assert!(error.contains(&left[0]), "{error}");
        assert_eq!(
            fs::read_to_string(tmp.path().join(&left[0])).unwrap(),
            broken
        );
    }

    /// `project.toml` names the root first and holds the layout under
    /// `[layout]`; reading it back gives the layout.
    #[test]
    fn the_project_file_holds_the_root_and_the_layout() {
        let layout = ProjectLayout {
            panel_groups: vec![],
            focused_group: 0,
        };
        let text = toml::to_string_pretty(&ProjectFileOut {
            root: Path::new("/home/u/proj"),
            layout: &layout,
        })
        .unwrap();
        assert!(text.starts_with("root = \"/home/u/proj\"\n"), "{text}");
        assert!(text.contains("[layout]"), "{text}");
        let back: ProjectFileIn = toml::from_str(&text).unwrap();
        assert_eq!(back.layout.unwrap().focused_group, 0);
    }

    #[derive(Serialize, Deserialize)]
    struct Panels {
        panels: Vec<PanelState>,
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

    #[test]
    fn project_key_mirrors_the_path_without_its_root() {
        assert_eq!(
            project_key(Path::new("/nonexistent/home/u/proj")),
            PathBuf::from("nonexistent/home/u/proj")
        );
    }
}
