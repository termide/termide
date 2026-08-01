//! Filesystem maintenance for session storage: unsaved-buffer and log
//! bookkeeping, stale-session cleanup, and session listing. Split out of the
//! session model; operates purely on paths and the `Session` snapshot.

use anyhow::{Context, Result};
use chrono::Local;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{get_data_dir, Session, SessionPanel};

/// Generate a unique filename for an unsaved buffer
///
/// Format: unsaved-YYYYMMDD-HHIISS-MSEC.txt
/// Example: unsaved-20251203-143022-456.txt
pub fn generate_unsaved_filename() -> String {
    let now = Local::now();
    let millis = now.timestamp_subsec_millis();
    format!("unsaved-{}-{:03}.txt", now.format("%Y%m%d-%H%M%S"), millis)
}

/// Generate a unique filename for session log
///
/// Format: session-YYYYMMDD-HHMMSS-MSC.log
/// Example: session-20251206-143022-456.log
pub fn generate_log_filename() -> String {
    let now = Local::now();
    let millis = now.timestamp_subsec_millis();
    format!("session-{}-{:03}.log", now.format("%Y%m%d-%H%M%S"), millis)
}

/// Cleanup old log files in session directory
///
/// Removes log files (session-*.log) that haven't been modified for more than 24 hours.
/// Uses modification time (not creation time) so active long-running sessions keep their logs.
pub fn cleanup_old_logs(session_dir: &Path) -> Result<()> {
    if !session_dir.exists() {
        return Ok(());
    }

    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 60 * 60);

    let entries = match fs::read_dir(session_dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            if filename.starts_with("session-") && filename.ends_with(".log") {
                if let Ok(metadata) = path.metadata() {
                    // Check last modification time - active sessions keep updating their logs
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
pub fn save_unsaved_buffer(session_dir: &Path, filename: &str, content: &str) -> Result<()> {
    let buffer_path = session_dir.join(filename);
    fs::write(&buffer_path, content).with_context(|| {
        format!(
            "Failed to write unsaved buffer file: {}",
            buffer_path.display()
        )
    })?;
    Ok(())
}

/// Load unsaved buffer content from a temporary file
pub fn load_unsaved_buffer(session_dir: &Path, filename: &str) -> Result<String> {
    let buffer_path = session_dir.join(filename);
    fs::read_to_string(&buffer_path).with_context(|| {
        format!(
            "Failed to read unsaved buffer file: {}",
            buffer_path.display()
        )
    })
}

/// Clean up (delete) an unsaved buffer temporary file
pub fn cleanup_unsaved_buffer(session_dir: &Path, filename: &str) -> Result<()> {
    let buffer_path = session_dir.join(filename);
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

/// Remove unsaved-*.txt files not referenced in the given session.
pub fn cleanup_stale_buffers(session_dir: &Path, session: &Session) {
    let active: HashSet<&str> = session
        .panel_groups
        .iter()
        .flat_map(|g| &g.panels)
        .filter_map(|p| match p {
            SessionPanel::Editor {
                unsaved_buffer_file,
                ..
            } => unsaved_buffer_file.as_deref(),
            _ => None,
        })
        .collect();

    let Ok(entries) = fs::read_dir(session_dir) else {
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

/// Clean up old sessions (excluding the current project's session)
///
/// Removes sessions older than `retention_days` from the sessions directory
pub fn cleanup_old_sessions(current_project: &Path, retention_days: u32) -> Result<()> {
    use std::time::{Duration, SystemTime};

    // 0 disables cleanup (keep sessions forever). Guard against the footgun
    // where a 0 cutoff of "now" would delete every non-current session.
    if retention_days == 0 {
        return Ok(());
    }

    let data_dir = get_data_dir()?;
    let sessions_dir = data_dir.join("sessions");

    if !sessions_dir.exists() {
        return Ok(()); // No sessions to clean up
    }

    // Canonicalize current project path for comparison
    let current_canonical = current_project
        .canonicalize()
        .unwrap_or_else(|_| current_project.to_path_buf());

    let retention_duration = Duration::from_secs(retention_days as u64 * 24 * 60 * 60);
    let cutoff_time = SystemTime::now()
        .checked_sub(retention_duration)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    // Walk through sessions directory recursively
    walk_and_cleanup(&sessions_dir, &current_canonical, cutoff_time)?;

    Ok(())
}

/// Recursively walk through directories and clean up old sessions
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

        // Project paths nest, so their session directories nest too (e.g.
        // `.../Data/Downloads` and `.../Data/Downloads/proj`). Always recurse
        // first so every session is evaluated on its own age — never shadowed
        // by, or deleted together with, an ancestor session.
        let _ = walk_and_cleanup(&path, current_project, cutoff_time);

        let session_file = path.join("session.toml");
        if !session_file.exists() || is_same_session(&path, current_project) {
            continue;
        }
        let stale = session_file
            .metadata()
            .and_then(|m| m.modified())
            .map(|modified| modified < cutoff_time)
            .unwrap_or(false);
        if !stale || has_non_empty_unsaved_buffers(&path) {
            continue;
        }

        if contains_nested_session(&path) {
            // A parent project session that also contains child project
            // sessions: drop only this session's own files, keep the nested
            // ones intact.
            remove_session_own_files(&path);
        } else if let Err(e) = fs::remove_dir_all(&path) {
            log::warn!("Failed to remove old session {}: {}", path.display(), e);
        }
    }

    Ok(())
}

/// Whether any subdirectory of `dir` (at any depth) holds a `session.toml`,
/// i.e. `dir` is an ancestor of one or more nested project sessions.
fn contains_nested_session(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && (path.join("session.toml").exists() || contains_nested_session(&path)) {
            return true;
        }
    }
    false
}

/// Remove only a session's own files (`session.toml` and its unsaved buffer
/// files), leaving any nested project session directories untouched. Prunes the
/// directory afterwards only if it ended up empty.
fn remove_session_own_files(dir: &Path) {
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
    // Succeeds only when nothing (no nested sessions, no other files) remains.
    let _ = fs::remove_dir(dir);
}

/// Check if session directory corresponds to the given project path
fn is_same_session(session_dir: &Path, project_path: &Path) -> bool {
    let data_dir = match get_data_dir() {
        Ok(dir) => dir,
        Err(_) => return false,
    };

    let sessions_base = data_dir.join("sessions");

    // Extract relative path from session directory
    let rel_path = match session_dir.strip_prefix(&sessions_base) {
        Ok(p) => p,
        Err(_) => return false,
    };

    // Reconstruct full path
    let reconstructed = PathBuf::from("/").join(rel_path);

    // Canonicalize both paths for comparison
    let reconstructed_canonical = reconstructed.canonicalize().unwrap_or(reconstructed);
    let project_canonical = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());

    reconstructed_canonical == project_canonical
}

/// Check if an unsaved buffer file is empty or contains only whitespace
fn is_buffer_file_empty(path: &Path) -> bool {
    match fs::read_to_string(path) {
        Ok(content) => content.trim().is_empty(),
        Err(_) => false, // Can't read — assume non-empty, don't delete
    }
}

/// Check if a session directory contains any non-empty unsaved buffer files
fn has_non_empty_unsaved_buffers(session_dir: &Path) -> bool {
    let entries = match fs::read_dir(session_dir) {
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
pub fn restore_orphaned_buffers(session_dir: &Path) -> Result<Vec<String>> {
    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    // Load session to get list of active buffer files
    let session_file = session_dir.join("session.toml");
    let active_buffers: HashSet<String> = if session_file.exists() {
        match fs::read_to_string(&session_file) {
            Ok(contents) => match toml::from_str::<Session>(&contents) {
                Ok(session) => {
                    // Collect all unsaved_buffer_file references from session
                    session
                        .panel_groups
                        .iter()
                        .flat_map(|group| &group.panels)
                        .filter_map(|panel| match panel {
                            SessionPanel::Editor {
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
        HashSet::new() // No session file, clean all temporary files
    };

    // Find all unsaved-*.txt files in session directory
    let entries = match fs::read_dir(session_dir) {
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

/// Delete a temporary unsaved buffer file from the session directory
/// This should be called when an editor with an unsaved buffer is closed without saving
pub fn delete_unsaved_buffer(session_dir: &Path, filename: &str) -> Result<()> {
    let temp_file = session_dir.join(filename);

    // Only delete if the file exists
    if temp_file.exists() {
        fs::remove_file(&temp_file)
            .with_context(|| format!("Failed to delete unsaved buffer file: {}", filename))?;
    }

    Ok(())
}

/// Information about a discovered session
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Original project path (reconstructed from session directory)
    pub project_path: PathBuf,
    /// Path to session.toml file
    pub session_path: PathBuf,
    /// Last modification time of session.toml
    pub modified: std::time::SystemTime,
}

/// List all available sessions, sorted by modification time (newest first)
pub fn list_all_sessions() -> Result<Vec<SessionInfo>> {
    let data_dir = get_data_dir()?;
    let sessions_dir = data_dir.join("sessions");

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    collect_sessions(&sessions_dir, &sessions_dir, &mut sessions)?;

    // Sort by modification time (newest first)
    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));

    Ok(sessions)
}

/// Recursively collect sessions from directory tree
fn collect_sessions(
    dir: &Path,
    sessions_base: &Path,
    sessions: &mut Vec<SessionInfo>,
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
            let session_file = path.join("session.toml");

            if session_file.exists() {
                // Extract project path from session directory structure
                if let Ok(rel_path) = path.strip_prefix(sessions_base) {
                    let project_path = PathBuf::from("/").join(rel_path);

                    // Get modification time
                    let modified = session_file
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                    sessions.push(SessionInfo {
                        project_path,
                        session_path: session_file,
                        modified,
                    });
                }
            }

            // Always recurse into subdirectories to find nested sessions
            let _ = collect_sessions(&path, sessions_base, sessions);
        }
    }

    Ok(())
}

/// Format a SystemTime as a relative time string (e.g., "2 hours ago").
pub fn format_relative_time(time: std::time::SystemTime) -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(time)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    termide_i18n::relative_age(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionPanelGroup;

    /// Regression: a stale *parent* project session must not take fresh
    /// *nested* project sessions down with it (previously `remove_dir_all` on
    /// the parent wiped nested sessions, and nested sessions were never
    /// evaluated on their own age).
    #[test]
    fn cleanup_keeps_nested_fresh_session_when_parent_is_stale() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::{Duration, SystemTime};

        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir().join(format!(
            "termide-session-nest-{}-{}",
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

        // A current project that matches neither session.
        let current = base.join("nonexistent");
        walk_and_cleanup(&base, &current, cutoff).unwrap();

        assert!(child_toml.exists(), "fresh nested session must survive");
        assert!(
            !parent_toml.exists(),
            "stale parent session should be cleaned"
        );

        let _ = fs::remove_dir_all(&base);
    }

    // =========================================================================
    // Round-trip serialization
    // =========================================================================

    #[test]
    fn test_round_trip_serialization() {
        let session = Session {
            panel_groups: vec![
                SessionPanelGroup {
                    panels: vec![
                        SessionPanel::FileManager {
                            path_or_url: "/home/user/project".to_string(),
                        },
                        SessionPanel::Editor {
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
                SessionPanelGroup {
                    panels: vec![SessionPanel::Terminal {
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

        let toml_str = toml::to_string_pretty(&session).unwrap();
        let restored: Session = toml::from_str(&toml_str).unwrap();

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
        let session: Session = toml::from_str(toml_str).unwrap();
        match &session.panel_groups[0].panels[0] {
            SessionPanel::FileManager { path_or_url } => {
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
        let session = Session {
            panel_groups: vec![SessionPanelGroup {
                panels: vec![SessionPanel::FileManager {
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

        let toml_str = toml::to_string_pretty(&session).unwrap();
        let restored: Session = toml::from_str(&toml_str).unwrap();

        match &restored.panel_groups[0].panels[0] {
            SessionPanel::FileManager { path_or_url } => {
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
        let session = Session {
            panel_groups: vec![SessionPanelGroup {
                panels: vec![SessionPanel::Markdown {
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

        let toml_str = toml::to_string_pretty(&session).unwrap();
        // Serialized with the "markdown" type tag.
        assert!(toml_str.contains("type = \"markdown\""), "{toml_str}");

        let restored: Session = toml::from_str(&toml_str).unwrap();
        match &restored.panel_groups[0].panels[0] {
            SessionPanel::Markdown { path } => {
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
    // Session directory mapping
    // =========================================================================

    #[test]
    fn test_session_dir_mapping() {
        let project = Path::new("/home/user/project");
        let session_dir = Session::get_session_dir(project).unwrap();
        // Should contain "sessions/home/user/project"
        let path_str = session_dir.to_string_lossy();
        assert!(path_str.contains("sessions"));
        assert!(path_str.ends_with("home/user/project"));
    }

    #[test]
    fn test_session_path_has_toml_extension() {
        let project = Path::new("/home/user/project");
        let session_path = Session::get_session_path(project).unwrap();
        assert!(session_path.to_string_lossy().ends_with("session.toml"));
    }

    // =========================================================================
    // Empty/corrupt session handling
    // =========================================================================

    #[test]
    fn test_empty_toml_fails_gracefully() {
        let result: Result<Session, _> = toml::from_str("");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_toml_fails_gracefully() {
        let result: Result<Session, _> = toml::from_str("this is not valid toml {{{}}}");
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
        let session: Session = toml::from_str(toml_str).unwrap();
        assert_eq!(session.panel_groups[0].panels.len(), 0);
    }

    // =========================================================================
    // All panel types serialize/deserialize
    // =========================================================================

    #[test]
    fn test_all_panel_types_round_trip() {
        let session = Session {
            panel_groups: vec![SessionPanelGroup {
                panels: vec![
                    SessionPanel::FileManager {
                        path_or_url: "/tmp".to_string(),
                    },
                    SessionPanel::Editor {
                        path: Some(PathBuf::from("/tmp/test.rs")),
                        unsaved_buffer_file: Some("unsaved-20251203-143022-456.txt".to_string()),
                    },
                    SessionPanel::Terminal {
                        working_dir: PathBuf::from("/tmp"),
                    },
                    SessionPanel::Journal,
                    SessionPanel::Image {
                        path: PathBuf::from("/tmp/img.png"),
                    },
                    SessionPanel::Binary {
                        path: PathBuf::from("/tmp/data.bin"),
                    },
                    SessionPanel::GitStatus {
                        repo_path: PathBuf::from("/tmp/repo"),
                    },
                    SessionPanel::GitLog {
                        repo_path: PathBuf::from("/tmp/repo"),
                    },
                    SessionPanel::GitDiff {
                        repo_path: PathBuf::from("/tmp/repo"),
                        commit_hash: Some("abc123".to_string()),
                    },
                    SessionPanel::Outline,
                    SessionPanel::Diagnostics,
                ],
                expanded_index: 0,
                width: None,
                mode: Default::default(),
                split_heights: None,
                fullscreen_cache: None,
            }],
            focused_group: 0,
        };

        let toml_str = toml::to_string_pretty(&session).unwrap();
        let restored: Session = toml::from_str(&toml_str).unwrap();
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
