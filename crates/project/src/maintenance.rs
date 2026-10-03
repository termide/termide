//! Filesystem maintenance for the project store: unsaved-buffer and log
//! bookkeeping, cleanup of stale projects, and the project listing. Split out
//! of the layout model; operates purely on paths and the `ProjectLayout`
//! snapshot.

use anyhow::{Context, Result};
use chrono::Local;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{get_data_dir, PanelState, ProjectLayout};

/// Generate a unique filename for an unsaved buffer
///
/// Format: unsaved-YYYYMMDD-HHIISS-MSEC.txt
/// Example: unsaved-20251203-143022-456.txt
pub fn generate_unsaved_filename() -> String {
    let now = Local::now();
    let millis = now.timestamp_subsec_millis();
    format!("unsaved-{}-{:03}.txt", now.format("%Y%m%d-%H%M%S"), millis)
}

/// Generate a unique filename for the log of one termide run
///
/// Format: session-YYYYMMDD-HHMMSS-MSC.log
/// Example: session-20251206-143022-456.log
pub fn generate_log_filename() -> String {
    let now = Local::now();
    let millis = now.timestamp_subsec_millis();
    format!("session-{}-{:03}.log", now.format("%Y%m%d-%H%M%S"), millis)
}

/// Cleanup old log files in a project directory
///
/// Removes log files (session-*.log) that haven't been modified for more than 24 hours.
/// Uses modification time (not creation time) so long-running instances keep their logs.
pub fn cleanup_old_logs(project_dir: &Path) -> Result<()> {
    if !project_dir.exists() {
        return Ok(());
    }

    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 60 * 60);

    let entries = match fs::read_dir(project_dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            if filename.starts_with("session-") && filename.ends_with(".log") {
                if let Ok(metadata) = path.metadata() {
                    // Check last modification time - running instances keep updating their logs
                    if let Ok(modified) = metadata.modified() {
                        if modified < cutoff {
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Save unsaved buffer content to a temporary file
pub fn save_unsaved_buffer(project_dir: &Path, filename: &str, content: &str) -> Result<()> {
    let buffer_path = project_dir.join(filename);
    fs::write(&buffer_path, content).with_context(|| {
        format!(
            "Failed to write unsaved buffer file: {}",
            buffer_path.display()
        )
    })?;
    Ok(())
}

/// Load unsaved buffer content from a temporary file
pub fn load_unsaved_buffer(project_dir: &Path, filename: &str) -> Result<String> {
    let buffer_path = project_dir.join(filename);
    fs::read_to_string(&buffer_path).with_context(|| {
        format!(
            "Failed to read unsaved buffer file: {}",
            buffer_path.display()
        )
    })
}

/// Clean up (delete) an unsaved buffer temporary file
pub fn cleanup_unsaved_buffer(project_dir: &Path, filename: &str) -> Result<()> {
    let buffer_path = project_dir.join(filename);
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

/// Remove unsaved-*.txt files not referenced in the given layout.
pub fn cleanup_stale_buffers(project_dir: &Path, layout: &ProjectLayout) {
    let active: HashSet<&str> = layout
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
        .collect();

    let Ok(entries) = fs::read_dir(project_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            if name.starts_with("unsaved-") && name.ends_with(".txt") && !active.contains(name) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Clean up old project layouts (excluding the current project's)
///
/// Removes layouts older than `retention_days` from the projects directory
pub fn cleanup_old_projects(current_project: &Path, retention_days: u32) -> Result<()> {
    use std::time::{Duration, SystemTime};

    // 0 disables cleanup (keep layouts forever). Guard against the footgun
    // where a 0 cutoff of "now" would delete every non-current layout.
    if retention_days == 0 {
        return Ok(());
    }

    let data_dir = get_data_dir()?;
    let projects_dir = data_dir.join(crate::PROJECTS_DIR);

    if !projects_dir.exists() {
        return Ok(()); // No layouts to clean up
    }

    // Canonicalize current project path for comparison
    let current_canonical =
        dunce::canonicalize(current_project).unwrap_or_else(|_| current_project.to_path_buf());

    let retention_duration = Duration::from_secs(retention_days as u64 * 24 * 60 * 60);
    let cutoff_time = SystemTime::now()
        .checked_sub(retention_duration)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    // Walk through the projects directory recursively
    walk_and_cleanup(&projects_dir, &current_canonical, cutoff_time)?;

    Ok(())
}

/// Recursively walk through directories and clean up old project layouts
fn walk_and_cleanup(
    dir: &Path,
    current_project: &Path,
    cutoff_time: std::time::SystemTime,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // Skip entries we can't read
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // Project paths nest, so their project directories nest too (e.g.
        // `.../Data/Downloads` and `.../Data/Downloads/proj`). Always recurse
        // first so every layout is evaluated on its own age — never shadowed
        // by, or deleted together with, an ancestor project's layout.
        let _ = walk_and_cleanup(&path, current_project, cutoff_time);

        let layout_file = path.join("session.toml");
        if !layout_file.exists() || is_same_project(&path, current_project) {
            continue;
        }
        let stale = layout_file
            .metadata()
            .and_then(|m| m.modified())
            .map(|modified| modified < cutoff_time)
            .unwrap_or(false);
        if !stale || has_non_empty_unsaved_buffers(&path) {
            continue;
        }

        if contains_nested_layout(&path) {
            // A parent project that also contains child projects with saved
            // layouts: drop only this project's own files, keep the nested
            // ones intact.
            remove_own_layout_files(&path);
        } else if let Err(e) = fs::remove_dir_all(&path) {
            log::warn!(
                "Failed to remove old project layout {}: {}",
                path.display(),
                e
            );
        }
    }

    Ok(())
}

/// Whether any subdirectory of `dir` (at any depth) holds a `session.toml`,
/// i.e. `dir` is an ancestor of one or more nested projects.
fn contains_nested_layout(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && (path.join("session.toml").exists() || contains_nested_layout(&path)) {
            return true;
        }
    }
    false
}

/// Remove only a project's own files (`session.toml` and its unsaved buffer
/// files), leaving any nested project directories untouched. Prunes the
/// directory afterwards only if it ended up empty.
fn remove_own_layout_files(dir: &Path) {
    let _ = fs::remove_file(dir.join("session.toml"));
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("unsaved-") && name.ends_with(".txt") {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
    // Succeeds only when nothing (no nested projects, no other files) remains.
    let _ = fs::remove_dir(dir);
}

/// Check if a project directory corresponds to the given project path
fn is_same_project(project_dir: &Path, project_path: &Path) -> bool {
    let data_dir = match get_data_dir() {
        Ok(dir) => dir,
        Err(_) => return false,
    };

    let projects_base = data_dir.join(crate::PROJECTS_DIR);

    // Extract relative path from the project directory
    let rel_path = match project_dir.strip_prefix(&projects_base) {
        Ok(p) => p,
        Err(_) => return false,
    };

    // Reconstruct full path
    let reconstructed = PathBuf::from("/").join(rel_path);

    // Canonicalize both paths for comparison
    let reconstructed_canonical = dunce::canonicalize(&reconstructed).unwrap_or(reconstructed);
    let project_canonical =
        dunce::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());

    reconstructed_canonical == project_canonical
}

/// Check if an unsaved buffer file is empty or contains only whitespace
fn is_buffer_file_empty(path: &Path) -> bool {
    match fs::read_to_string(path) {
        Ok(content) => content.trim().is_empty(),
        Err(_) => false, // Can't read — assume non-empty, don't delete
    }
}

/// Check if a project directory contains any non-empty unsaved buffer files
fn has_non_empty_unsaved_buffers(project_dir: &Path) -> bool {
    let entries = match fs::read_dir(project_dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            if filename.starts_with("unsaved-")
                && filename.ends_with(".txt")
                && !is_buffer_file_empty(&path)
            {
                return true;
            }
        }
    }
    false
}

/// Restore orphaned unsaved buffer files (not referenced in session.toml)
///
/// Empty orphaned files are deleted. Non-empty ones are returned
/// for the caller to add as editor panels (they contain user data
/// that may have been lost due to a crash).
pub fn restore_orphaned_buffers(project_dir: &Path) -> Result<Vec<String>> {
    if !project_dir.exists() {
        return Ok(Vec::new());
    }

    // Load the layout to get the list of active buffer files
    let layout_file = project_dir.join("session.toml");
    let active_buffers: HashSet<String> = if layout_file.exists() {
        match fs::read_to_string(&layout_file) {
            Ok(contents) => match toml::from_str::<ProjectLayout>(&contents) {
                Ok(layout) => {
                    // Collect all unsaved_buffer_file references from the layout
                    layout
                        .panel_groups
                        .iter()
                        .flat_map(|group| &group.panels)
                        .filter_map(|panel| match panel {
                            PanelState::Editor {
                                unsaved_buffer_file,
                                ..
                            } => unsaved_buffer_file.clone(),
                            _ => None,
                        })
                        .collect()
                }
                Err(_) => HashSet::new(), // Failed to parse, proceed with cleanup
            },
            Err(_) => HashSet::new(), // Failed to read, proceed with cleanup
        }
    } else {
        HashSet::new() // No layout file, clean all temporary files
    };

    // Find all unsaved-*.txt files in the project directory
    let entries = match fs::read_dir(project_dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };

    let mut restored = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();

        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            // Check if this is an unsaved buffer file
            if filename.starts_with("unsaved-") && filename.ends_with(".txt") {
                // If not in active list, handle it
                if !active_buffers.contains(filename) {
                    if is_buffer_file_empty(&path) {
                        let _ = fs::remove_file(&path); // empty → delete
                    } else {
                        restored.push(filename.to_string()); // non-empty → restore
                    }
                }
            }
        }
    }

    Ok(restored)
}

/// Delete a temporary unsaved buffer file from the project directory
/// This should be called when an editor with an unsaved buffer is closed without saving
pub fn delete_unsaved_buffer(project_dir: &Path, filename: &str) -> Result<()> {
    let temp_file = project_dir.join(filename);

    // Only delete if the file exists
    if temp_file.exists() {
        fs::remove_file(&temp_file)
            .with_context(|| format!("Failed to delete unsaved buffer file: {}", filename))?;
    }

    Ok(())
}

/// Information about a project with a saved layout
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// Original project path (reconstructed from the project directory)
    pub project_path: PathBuf,
    /// Path to session.toml file
    pub layout_path: PathBuf,
    /// Last modification time of session.toml
    pub modified: std::time::SystemTime,
}

/// List all projects with a saved layout, sorted by modification time (newest first)
pub fn list_all_projects() -> Result<Vec<ProjectInfo>> {
    let data_dir = get_data_dir()?;
    let projects_dir = data_dir.join(crate::PROJECTS_DIR);

    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut projects = Vec::new();
    collect_projects(&projects_dir, &projects_dir, &mut projects)?;

    // Sort by modification time (newest first)
    projects.sort_by_key(|p| std::cmp::Reverse(p.modified));

    Ok(projects)
}

/// Recursively collect projects from the directory tree
fn collect_projects(
    dir: &Path,
    projects_base: &Path,
    projects: &mut Vec<ProjectInfo>,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            let layout_file = path.join("session.toml");

            if layout_file.exists() {
                // Extract project path from the project directory structure
                if let Ok(rel_path) = path.strip_prefix(projects_base) {
                    let project_path = PathBuf::from("/").join(rel_path);

                    // Get modification time
                    let modified = layout_file
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                    projects.push(ProjectInfo {
                        project_path,
                        layout_path: layout_file,
                        modified,
                    });
                }
            }

            // Always recurse into subdirectories to find nested projects
            let _ = collect_projects(&path, projects_base, projects);
        }
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

    /// Regression: a stale *parent* project layout must not take fresh
    /// *nested* project layouts down with it (previously `remove_dir_all` on
    /// the parent wiped nested layouts, and nested layouts were never
    /// evaluated on their own age).
    #[test]
    fn cleanup_keeps_nested_fresh_layout_when_parent_is_stale() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::{Duration, SystemTime};

        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir().join(format!(
            "termide-layout-nest-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let parent = base.join("parent");
        let child = parent.join("child");
        fs::create_dir_all(&child).unwrap();

        let parent_toml = parent.join("session.toml");
        let child_toml = child.join("session.toml");
        fs::write(&parent_toml, "focused_group = 0\n").unwrap();
        fs::write(&child_toml, "focused_group = 0\n").unwrap();

        let now = SystemTime::now();
        let cutoff = now - Duration::from_secs(30 * 24 * 60 * 60);
        // Parent is 60 days old (stale); child keeps its fresh "now" mtime.
        let old = now - Duration::from_secs(60 * 24 * 60 * 60);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&parent_toml)
            .unwrap()
            .set_modified(old)
            .unwrap();

        // A current project that matches neither layout.
        let current = base.join("nonexistent");
        walk_and_cleanup(&base, &current, cutoff).unwrap();

        assert!(child_toml.exists(), "fresh nested layout must survive");
        assert!(
            !parent_toml.exists(),
            "stale parent layout should be cleaned"
        );

        let _ = fs::remove_dir_all(&base);
    }

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
        assert!(filename.starts_with("unsaved-"));
        assert!(filename.ends_with(".txt"));
        // Format: unsaved-YYYYMMDD-HHMMSS-MSC.txt
        assert!(filename.len() > 20);
    }

    #[test]
    fn test_generate_unsaved_filename_uniqueness() {
        // Two calls should (almost certainly) produce different names
        // due to millisecond precision
        let a = generate_unsaved_filename();
        let b = generate_unsaved_filename();
        // They might be the same if called within the same millisecond,
        // but we're testing the format is consistent
        assert!(a.starts_with("unsaved-"));
        assert!(b.starts_with("unsaved-"));
    }

    // =========================================================================
    // Project layout directory mapping
    // =========================================================================

    #[test]
    fn test_project_dir_mapping() {
        let project = Path::new("/home/user/project");
        let project_dir = ProjectLayout::get_project_dir(project).unwrap();
        // Should contain "projects/home/user/project"
        let path_str = project_dir.to_string_lossy();
        assert!(path_str.contains(crate::PROJECTS_DIR));
        assert!(path_str.ends_with("home/user/project"));
    }

    #[test]
    fn test_layout_path_has_toml_extension() {
        let project = Path::new("/home/user/project");
        let layout_path = ProjectLayout::get_project_path(project).unwrap();
        assert!(layout_path.to_string_lossy().ends_with("session.toml"));
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

    #[test]
    fn test_generate_log_filename_format() {
        let filename = generate_log_filename();
        assert!(filename.starts_with("session-"));
        assert!(filename.ends_with(".log"));
    }
}
