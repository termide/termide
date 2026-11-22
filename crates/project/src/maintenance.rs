//! Filesystem maintenance in a project's directory of the store: its
//! unsaved buffers (`buffers/`) and run logs (`logs/`). Operates purely on
//! paths and the `ProjectLayout` snapshot.

use anyhow::{Context, Result};
use chrono::Local;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::store::{BUFFERS_DIR, LOGS_DIR, PROJECT_FILE};
use crate::{PanelState, ProjectFileIn, ProjectLayout};

/// How long a run log is kept after its last write.
const LOG_RETENTION: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// A name for a file made now: `YYYYMMDD-HHMMSS-MSC` and `extension`.
fn stamped(extension: &str) -> String {
    let now = Local::now();
    let millis = now.timestamp_subsec_millis();
    format!("{}-{millis:03}.{extension}", now.format("%Y%m%d-%H%M%S"))
}

/// Generate a unique filename for an unsaved buffer, kept in the project's
/// `buffers/`.
///
/// Format: YYYYMMDD-HHMMSS-MSC-PID-N.txt
/// Example: 20251203-143022-456-4242-0.txt
pub fn generate_unsaved_filename() -> String {
    // Buffers named in one pass share the millisecond: a count kept for
    // the process tells them apart, and the process id tells this
    // termide's from another's.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    let name = stamped("txt");
    let stem = name.trim_end_matches(".txt");
    format!("{stem}-{}-{n}.txt", std::process::id())
}

/// Generate a unique filename for the log of one termide run.
///
/// Format: YYYYMMDD-HHMMSS-MSC.log
/// Example: 20251206-143022-456.log
pub fn generate_log_filename() -> String {
    stamped("log")
}

/// Where the log of a termide run started in the project kept in
/// `project_dir` goes: a new file in its `logs/`, created as needed.
pub fn new_log_path(project_dir: &Path) -> PathBuf {
    let logs = project_dir.join(LOGS_DIR);
    let _ = fs::create_dir_all(&logs);
    logs.join(generate_log_filename())
}

/// Remove the run logs in the project directory `project_dir` not written
/// to for a day. Modification time, not creation time, so long-running
/// instances keep their logs.
pub fn cleanup_old_logs(project_dir: &Path) {
    let Some(cutoff) = std::time::SystemTime::now().checked_sub(LOG_RETENTION) else {
        return;
    };
    let Ok(entries) = fs::read_dir(project_dir.join(LOGS_DIR)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_log = path.extension().is_some_and(|ext| ext == "log");
        let old = path
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified < cutoff);
        if is_log && old {
            let _ = fs::remove_file(&path);
        }
    }
}

/// The file of the unsaved buffer `filename` in the project directory
/// `project_dir`.
fn buffer_path(project_dir: &Path, filename: &str) -> PathBuf {
    project_dir.join(BUFFERS_DIR).join(filename)
}

/// Whether `name` is the name of an unsaved buffer file.
fn is_buffer_name(name: &str) -> bool {
    name.ends_with(".txt")
}

/// Save unsaved buffer content to its backup file in the project directory,
/// replacing the previous backup atomically.
pub fn save_unsaved_buffer(project_dir: &Path, filename: &str, content: &str) -> Result<()> {
    // The layout is saved after its buffers, so on a project's first save
    // the directory may not be there yet.
    let buffers = project_dir.join(BUFFERS_DIR);
    fs::create_dir_all(&buffers)
        .with_context(|| format!("Failed to create buffer directory: {}", buffers.display()))?;
    let buffer_path = buffers.join(filename);
    termide_fsutil::write_atomic(&buffer_path, content.as_bytes()).with_context(|| {
        format!(
            "Failed to write unsaved buffer file: {}",
            buffer_path.display()
        )
    })?;
    Ok(())
}

/// Whether the backup file `filename` of an unsaved buffer is there: kept
/// when writing a newer one failed, an older copy of the changes beats none.
pub fn has_unsaved_buffer(project_dir: &Path, filename: &str) -> bool {
    buffer_path(project_dir, filename).is_file()
}

/// Load unsaved buffer content from its backup file
pub fn load_unsaved_buffer(project_dir: &Path, filename: &str) -> Result<String> {
    let buffer_path = buffer_path(project_dir, filename);
    fs::read_to_string(&buffer_path).with_context(|| {
        format!(
            "Failed to read unsaved buffer file: {}",
            buffer_path.display()
        )
    })
}

/// Clean up (delete) an unsaved buffer backup file
pub fn cleanup_unsaved_buffer(project_dir: &Path, filename: &str) -> Result<()> {
    let buffer_path = buffer_path(project_dir, filename);
    if buffer_path.exists() {
        fs::remove_file(&buffer_path).with_context(|| {
            format!(
                "Failed to delete unsaved buffer file: {}",
                buffer_path.display()
            )
        })?;
    }
    Ok(())
}

/// The unsaved buffer files `layout` refers to.
fn buffers_of(layout: &ProjectLayout) -> HashSet<&str> {
    layout
        .panel_groups
        .iter()
        .flat_map(|g| &g.panels)
        .filter_map(|p| match p {
            PanelState::Editor {
                unsaved_buffer_file,
                ..
            } => unsaved_buffer_file.as_deref(),
            _ => None,
        })
        .collect()
}

/// Remove the buffer files not referenced in the given layout.
pub fn cleanup_stale_buffers(project_dir: &Path, layout: &ProjectLayout) {
    let active = buffers_of(layout);
    let Ok(entries) = fs::read_dir(project_dir.join(BUFFERS_DIR)) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            if is_buffer_name(name) && !active.contains(name) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Check if an unsaved buffer file is empty or contains only whitespace
fn is_buffer_file_empty(path: &Path) -> bool {
    match fs::read_to_string(path) {
        Ok(content) => content.trim().is_empty(),
        Err(_) => false, // Can't read — assume non-empty, don't delete
    }
}

/// Restore orphaned unsaved buffer files (not referenced in the layout)
///
/// Empty orphaned files are deleted. Non-empty ones are returned
/// for the caller to add as editor panels (they contain user data
/// that may have been lost due to a crash).
pub fn restore_orphaned_buffers(project_dir: &Path) -> Result<Vec<String>> {
    let layout = fs::read_to_string(project_dir.join(PROJECT_FILE))
        .ok()
        .and_then(|text| toml::from_str::<ProjectFileIn>(&text).ok())
        .and_then(|file| file.layout);
    let active = layout.as_ref().map(buffers_of).unwrap_or_default();

    let Ok(entries) = fs::read_dir(project_dir.join(BUFFERS_DIR)) else {
        return Ok(Vec::new());
    };
    let mut restored = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_buffer_name(filename) || active.contains(filename) {
            continue;
        }
        if is_buffer_file_empty(&path) {
            let _ = fs::remove_file(&path);
        } else {
            restored.push(filename.to_string());
        }
    }
    Ok(restored)
}

/// Delete an unsaved buffer backup file from the project directory.
/// This should be called when an editor with an unsaved buffer is closed without saving
pub fn delete_unsaved_buffer(project_dir: &Path, filename: &str) -> Result<()> {
    let temp_file = buffer_path(project_dir, filename);
    if temp_file.exists() {
        fs::remove_file(&temp_file)
            .with_context(|| format!("Failed to delete unsaved buffer file: {}", filename))?;
    }
    Ok(())
}

/// Format a SystemTime as the local date and minute (`2026-10-03 14:22`), as
/// the agent's session list shows times.
pub fn format_local_minute(time: std::time::SystemTime) -> String {
    chrono::DateTime::<Local>::from(time)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PanelGroupState;

    // =========================================================================
    // Round-trip serialization
    // =========================================================================

    #[test]
    fn test_round_trip_serialization() {
        let layout = ProjectLayout {
            panel_groups: vec![
                PanelGroupState {
                    panels: vec![
                        PanelState::FileManager {
                            path_or_url: "/home/user/project".to_string(),
                        },
                        PanelState::Editor {
                            path: Some(PathBuf::from("/home/user/project/main.rs")),
                            unsaved_buffer_file: None,
                        },
                    ],
                    expanded_index: 1,
                    mode: Default::default(),
                    split_heights: None,
                    fullscreen_cache: None,
                    width: Some(120),
                },
                PanelGroupState {
                    panels: vec![PanelState::Terminal {
                        working_dir: PathBuf::from("/home/user/project"),
                    }],
                    expanded_index: 0,
                    width: None,
                    mode: Default::default(),
                    split_heights: None,
                    fullscreen_cache: None,
                },
            ],
            focused_group: 0,
        };

        let toml_str = toml::to_string_pretty(&layout).unwrap();
        let restored: ProjectLayout = toml::from_str(&toml_str).unwrap();

        assert_eq!(restored.focused_group, 0);
        assert_eq!(restored.panel_groups.len(), 2);
        assert_eq!(restored.panel_groups[0].panels.len(), 2);
        assert_eq!(restored.panel_groups[0].expanded_index, 1);
        assert_eq!(restored.panel_groups[0].width, Some(120));
        assert_eq!(restored.panel_groups[1].width, None);
    }

    // =========================================================================
    // Backward compatibility — old "path" field alias
    // =========================================================================

    #[test]
    fn test_backward_compat_path_alias() {
        let toml_str = r#"
focused_group = 0

[[panel_groups]]
expanded_index = 0

[[panel_groups.panels]]
type = "file_manager"
path = "/old/style/path"
"#;
        let layout: ProjectLayout = toml::from_str(toml_str).unwrap();
        match &layout.panel_groups[0].panels[0] {
            PanelState::FileManager { path_or_url } => {
                assert_eq!(path_or_url, "/old/style/path");
            }
            _ => panic!("Expected FileManager panel"),
        }
    }

    // =========================================================================
    // Remote path preservation (SFTP URLs)
    // =========================================================================

    #[test]
    fn test_sftp_url_round_trip() {
        let layout = ProjectLayout {
            panel_groups: vec![PanelGroupState {
                panels: vec![PanelState::FileManager {
                    path_or_url: "sftp://user@host:22/remote/path".to_string(),
                }],
                expanded_index: 0,
                width: None,
                mode: Default::default(),
                split_heights: None,
                fullscreen_cache: None,
            }],
            focused_group: 0,
        };

        let toml_str = toml::to_string_pretty(&layout).unwrap();
        let restored: ProjectLayout = toml::from_str(&toml_str).unwrap();

        match &restored.panel_groups[0].panels[0] {
            PanelState::FileManager { path_or_url } => {
                assert_eq!(path_or_url, "sftp://user@host:22/remote/path");
            }
            _ => panic!("Expected FileManager panel"),
        }
    }

    // =========================================================================
    // Unsaved buffer file naming
    // =========================================================================

    #[test]
    fn test_markdown_panel_round_trip() {
        let layout = ProjectLayout {
            panel_groups: vec![PanelGroupState {
                panels: vec![PanelState::Markdown {
                    path: PathBuf::from("/home/user/project/README.md"),
                }],
                expanded_index: 0,
                mode: Default::default(),
                split_heights: None,
                fullscreen_cache: None,
                width: None,
            }],
            focused_group: 0,
        };

        let toml_str = toml::to_string_pretty(&layout).unwrap();
        // Serialized with the "markdown" type tag.
        assert!(toml_str.contains("type = \"markdown\""), "{toml_str}");

        let restored: ProjectLayout = toml::from_str(&toml_str).unwrap();
        match &restored.panel_groups[0].panels[0] {
            PanelState::Markdown { path } => {
                assert_eq!(path, &PathBuf::from("/home/user/project/README.md"));
            }
            other => panic!("expected Markdown panel, got {other:?}"),
        }
    }

    #[test]
    fn test_generate_unsaved_filename_format() {
        let filename = generate_unsaved_filename();
        assert!(filename.ends_with(".txt"));
        assert!(is_buffer_name(&filename));
        // Format: YYYYMMDD-HHMMSS-MSC-PID-N.txt
        let pid = format!("-{}-", std::process::id());
        assert!(filename.contains(&pid), "{filename}");
    }

    #[test]
    fn buffers_named_in_one_pass_get_names_of_their_own() {
        let names: HashSet<String> = (0..1000).map(|_| generate_unsaved_filename()).collect();
        assert_eq!(names.len(), 1000);
    }

    // =========================================================================
    // Project directory mapping
    // =========================================================================

    #[test]
    fn test_project_dir_mapping() {
        let project = Path::new("/home/user/project");
        let project_dir = ProjectLayout::get_project_dir(project).unwrap();
        assert_eq!(
            project_dir.parent().unwrap().file_name().unwrap(),
            crate::store::PROJECTS_DIR
        );
        assert_eq!(
            project_dir.file_name().unwrap().to_string_lossy(),
            crate::project_id(project)
        );
    }

    #[test]
    fn test_layout_path_is_the_project_file() {
        let project = Path::new("/home/user/project");
        let layout_path = ProjectLayout::get_project_path(project).unwrap();
        assert!(layout_path.ends_with(PROJECT_FILE));
    }

    // =========================================================================
    // Empty/corrupt layout handling
    // =========================================================================

    #[test]
    fn test_empty_toml_fails_gracefully() {
        let result: Result<ProjectLayout, _> = toml::from_str("");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_toml_fails_gracefully() {
        let result: Result<ProjectLayout, _> = toml::from_str("this is not valid toml {{{}}}");
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_panels_field() {
        let toml_str = r#"
focused_group = 0

[[panel_groups]]
expanded_index = 0
panels = []
"#;
        let layout: ProjectLayout = toml::from_str(toml_str).unwrap();
        assert_eq!(layout.panel_groups[0].panels.len(), 0);
    }

    // =========================================================================
    // All panel types serialize/deserialize
    // =========================================================================

    #[test]
    fn test_all_panel_types_round_trip() {
        let layout = ProjectLayout {
            panel_groups: vec![PanelGroupState {
                panels: vec![
                    PanelState::FileManager {
                        path_or_url: "/tmp".to_string(),
                    },
                    PanelState::Editor {
                        path: Some(PathBuf::from("/tmp/test.rs")),
                        unsaved_buffer_file: Some("unsaved-20251203-143022-456.txt".to_string()),
                    },
                    PanelState::Terminal {
                        working_dir: PathBuf::from("/tmp"),
                    },
                    PanelState::Journal,
                    PanelState::Image {
                        path: PathBuf::from("/tmp/img.png"),
                    },
                    PanelState::Binary {
                        path: PathBuf::from("/tmp/data.bin"),
                    },
                    PanelState::GitStatus {
                        repo_path: PathBuf::from("/tmp/repo"),
                    },
                    PanelState::GitLog {
                        repo_path: PathBuf::from("/tmp/repo"),
                    },
                    PanelState::GitDiff {
                        repo_path: PathBuf::from("/tmp/repo"),
                        commit_hash: Some("abc123".to_string()),
                    },
                    PanelState::Outline,
                    PanelState::Diagnostics,
                ],
                expanded_index: 0,
                width: None,
                mode: Default::default(),
                split_heights: None,
                fullscreen_cache: None,
            }],
            focused_group: 0,
        };

        let toml_str = toml::to_string_pretty(&layout).unwrap();
        let restored: ProjectLayout = toml::from_str(&toml_str).unwrap();
        assert_eq!(restored.panel_groups[0].panels.len(), 11);
    }

    // =========================================================================
    // Log filename generation
    // =========================================================================

    /// A project's first layout save writes its buffers before the layout
    /// creates the directory; the backup must not be lost to that order.
    #[test]
    fn an_unsaved_buffer_is_saved_into_a_missing_project_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("projects/home/u/new");
        save_unsaved_buffer(&project_dir, "unsaved-1.txt", "draft").unwrap();
        assert_eq!(
            load_unsaved_buffer(&project_dir, "unsaved-1.txt").unwrap(),
            "draft"
        );
        save_unsaved_buffer(&project_dir, "unsaved-1.txt", "second draft").unwrap();
        assert_eq!(
            load_unsaved_buffer(&project_dir, "unsaved-1.txt").unwrap(),
            "second draft"
        );
        let names: Vec<_> = fs::read_dir(project_dir.join(BUFFERS_DIR))
            .unwrap()
            .collect();
        assert_eq!(names.len(), 1, "no temporary file is left behind");
    }

    #[test]
    fn a_run_log_goes_to_the_logs_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let path = new_log_path(tmp.path());
        assert_eq!(path.parent().unwrap(), tmp.path().join(LOGS_DIR));
        assert!(path.parent().unwrap().is_dir());
        assert!(path.extension().is_some_and(|ext| ext == "log"));
    }

    /// Buffers the layout names stay, an orphan with content is handed back
    /// to be reopened, an empty orphan goes.
    #[test]
    fn orphaned_buffers_are_restored_or_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for (name, body) in [("kept.txt", "a"), ("orphan.txt", "b"), ("empty.txt", " ")] {
            save_unsaved_buffer(dir, name, body).unwrap();
        }
        let text = "root = \"/p\"\n[layout]\nfocused_group = 0\n[[layout.panel_groups]]\nexpanded_index = 0\n[[layout.panel_groups.panels]]\ntype = \"editor\"\nunsaved_buffer_file = \"kept.txt\"\n";
        fs::write(dir.join(PROJECT_FILE), text).unwrap();
        assert_eq!(restore_orphaned_buffers(dir).unwrap(), ["orphan.txt"]);
        assert!(dir.join(BUFFERS_DIR).join("kept.txt").exists());
        assert!(!dir.join(BUFFERS_DIR).join("empty.txt").exists());
    }
}
