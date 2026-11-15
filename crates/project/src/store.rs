//! The project store: one directory per project under `<data>/projects/`,
//! side by side, holding everything termide keeps of the project apart from
//! its own files.
//!
//! ```text
//! projects/<name>-<hash>/
//!   project.toml        the project's root and its saved layout
//!   trust.toml          what the user granted it: "allow always" rules and
//!                       the launches it brings that were approved
//!   owner.lock          held by the instance that has it open
//!   owner.toml          which instance that is
//!   logs/               one log per termide run started in it
//!   buffers/            unsaved editor buffers
//! ```
//!
//! A directory is named after the project's last path component, made safe
//! for every file system, and a hash of its canonical path: the name only
//! helps a person find it, the hash tells projects apart. The root is read
//! from `project.toml`, never rebuilt from the name, so no path has to
//! survive being turned into a file name.
//!
//! The store lives in termide's data directory, never in the project: a
//! repository's own files must not be able to grant anything.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::get_data_dir;
use crate::ownership::{claim_in, Owner, LOCK_FILE, OWNER_FILE};

/// The directory holding every project's directory.
pub(crate) const PROJECTS_DIR: &str = "projects";
/// The file naming a project's root and holding its layout.
pub(crate) const PROJECT_FILE: &str = "project.toml";
/// The file holding what the user granted a project.
pub(crate) const TRUST_FILE: &str = "trust.toml";
/// The directory of a project's run logs.
pub(crate) const LOGS_DIR: &str = "logs";
/// The directory of a project's unsaved buffers.
pub(crate) const BUFFERS_DIR: &str = "buffers";

/// How many characters of the project's name its directory keeps.
const NAME_CHARS: usize = 40;

/// The name of the directory the project at `root` is kept in:
/// `<name>-<hash>`, see the module documentation.
#[must_use]
pub fn project_id(root: &Path) -> String {
    let canonical = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let name: String = canonical
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        // Letters, digits, `-` and `_` only: no separators, nothing Windows
        // refuses, no dot to make a reserved device name (`CON.x`) or a
        // hidden directory.
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(NAME_CHARS)
        .collect();
    let name = if name.is_empty() { "root" } else { &name };
    let hash = fnv1a(canonical.to_string_lossy().as_bytes()) & 0xffff_ffff_ffff;
    format!("{name}-{hash:012x}")
}

/// FNV-1a, 64 bits: stable across platforms and releases, which the
/// standard library's hasher does not promise.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The directory holding the projects in the data directory `data_dir`.
pub(crate) fn projects_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join(PROJECTS_DIR)
}

/// The directory the project at `root` is kept in, under `data_dir`.
pub(crate) fn project_dir_in(data_dir: &Path, root: &Path) -> PathBuf {
    projects_dir_in(data_dir).join(project_id(root))
}

/// The directory the project at `root` is kept in.
///
/// # Errors
///
/// When there is no data directory.
pub fn project_dir(root: &Path) -> Result<PathBuf> {
    Ok(project_dir_in(&get_data_dir()?, root))
}

/// The file holding what the user granted the project at `root`: the
/// "allow always" permission rules, in the shape of `[ai.permissions]`, and
/// the launches it brings that were approved, under `[approved]` (see
/// `termide_agent_core::approvals`).
///
/// # Errors
///
/// When there is no data directory.
pub fn trust_path(root: &Path) -> Result<PathBuf> {
    Ok(project_dir(root)?.join(TRUST_FILE))
}

/// The root a project's `project.toml` names, read without its layout so
/// that a layout that no longer parses does not hide the project.
#[derive(Deserialize)]
struct RootOnly {
    root: PathBuf,
}

/// The root named by the `project.toml` in `dir`, if it names one.
pub(crate) fn read_root(dir: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(dir.join(PROJECT_FILE)).ok()?;
    toml::from_str::<RootOnly>(&text).ok().map(|file| file.root)
}

/// A project in the store.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// Its root, as its `project.toml` names it.
    pub project_path: PathBuf,
    /// Its directory in the store.
    pub dir: PathBuf,
    /// When its layout was last saved.
    pub modified: SystemTime,
}

/// Every project in the store, the most recently saved first.
///
/// # Errors
///
/// When there is no data directory.
pub fn list_all_projects() -> Result<Vec<ProjectInfo>> {
    Ok(list_in(&projects_dir_in(&get_data_dir()?)))
}

pub(crate) fn list_in(projects_dir: &Path) -> Vec<ProjectInfo> {
    let Ok(entries) = fs::read_dir(projects_dir) else {
        return Vec::new();
    };
    let mut projects: Vec<ProjectInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let root = read_root(&dir)?;
            let modified = fs::metadata(dir.join(PROJECT_FILE))
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some(ProjectInfo {
                project_path: root,
                dir,
                modified,
            })
        })
        .collect();
    projects.sort_by_key(|p| std::cmp::Reverse(p.modified));
    projects
}

/// The projects in the store whose roots lie inside `root`, `root` itself
/// left out.
pub(crate) fn nested_in(projects_dir: &Path, root: &Path) -> Vec<ProjectInfo> {
    list_in(projects_dir)
        .into_iter()
        .filter(|p| p.project_path != root && p.project_path.starts_with(root))
        .collect()
}

/// Why a project could not be deleted.
#[derive(Debug)]
pub enum DeleteError {
    /// Another instance has it open.
    Open(Owner),
    /// Its files could not be removed.
    Io(anyhow::Error),
}

impl std::fmt::Display for DeleteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(owner) => write!(f, "the project is open in process {}", owner.pid),
            Self::Io(e) => write!(f, "{e:#}"),
        }
    }
}

/// Delete everything the store keeps of the project at `root`: its layout,
/// buffers, logs and grants. The project's own files are not touched, nor
/// are the projects inside it.
///
/// The project is taken first, so one open in another instance is refused,
/// and its lock goes last, while still held: deleting a lock another
/// instance holds would let a third take the project beside it.
///
/// # Errors
///
/// [`DeleteError::Open`] when another instance has the project open,
/// [`DeleteError::Io`] when it cannot be removed.
pub fn delete_project(root: &Path) -> std::result::Result<(), DeleteError> {
    let dir = project_dir(root).map_err(DeleteError::Io)?;
    delete_dir(&dir)
}

pub(crate) fn delete_dir(dir: &Path) -> std::result::Result<(), DeleteError> {
    if !dir.exists() {
        return Ok(());
    }
    let claim = claim_in(dir, None).map_err(DeleteError::Open)?;
    remove_all_but_owner(dir)
        .with_context(|| format!("Failed to delete {}", dir.display()))
        .map_err(DeleteError::Io)?;
    remove_owner_files(dir);
    drop(claim);
    Ok(())
}

/// Remove everything in `dir` but its lock and owner note.
fn remove_all_but_owner(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == LOCK_FILE || name == OWNER_FILE {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Remove the lock and owner note of `dir`, then `dir` itself when that
/// leaves it empty. Only for the holder of the lock.
pub(crate) fn remove_owner_files(dir: &Path) {
    let _ = fs::remove_file(dir.join(OWNER_FILE));
    let _ = fs::remove_file(dir.join(LOCK_FILE));
    let _ = fs::remove_dir(dir);
}

/// Whether `dir` holds nothing but a lock and its owner note.
pub(crate) fn holds_only_owner(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().all(|entry| {
            let name = entry.file_name();
            name == LOCK_FILE || name == OWNER_FILE
        })
    })
}

/// Tidy the store at startup: drop the run logs older than a day in every
/// project, then the directories left with nothing a project needs — no
/// `project.toml`, no grants, no buffers — that no instance holds.
pub fn sweep_store() {
    if let Ok(data_dir) = get_data_dir() {
        sweep_in(&projects_dir_in(&data_dir));
    }
}

pub(crate) fn sweep_in(projects_dir: &Path) {
    let Ok(entries) = fs::read_dir(projects_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        crate::maintenance::cleanup_old_logs(&dir);
        if dir.join(PROJECT_FILE).exists() || dir.join(TRUST_FILE).exists() {
            continue;
        }
        let empty = |sub: &str| {
            fs::read_dir(dir.join(sub)).map_or(true, |mut entries| entries.next().is_none())
        };
        if empty(BUFFERS_DIR) && empty(LOGS_DIR) {
            // Refused when an instance holds it: a project opened and not
            // saved yet.
            let _ = delete_dir(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_id_is_a_safe_name_and_a_hash_of_the_path() {
        let id = project_id(Path::new("/nonexistent/home/u/my project.v2"));
        let (name, hash) = id.rsplit_once('-').unwrap();
        assert_eq!(name, "my_project_v2");
        assert_eq!(hash.len(), 12);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        // Stable, and told apart from a project of the same name elsewhere.
        assert_eq!(
            id,
            project_id(Path::new("/nonexistent/home/u/my project.v2"))
        );
        assert_ne!(
            id,
            project_id(Path::new("/nonexistent/other/my project.v2"))
        );
        assert!(project_id(Path::new("/")).starts_with("root-"));
        let long = format!("/nonexistent/{}", "x".repeat(300));
        assert_eq!(project_id(Path::new(&long)).len(), NAME_CHARS + 13);
    }

    fn write_project(projects: &Path, root: &str) -> PathBuf {
        let dir = projects.join(project_id(Path::new(root)));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(PROJECT_FILE), format!("root = {root:?}\n")).unwrap();
        dir
    }

    #[test]
    fn projects_are_listed_by_the_root_their_file_names() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join(PROJECTS_DIR);
        write_project(&projects, "/nonexistent/a");
        write_project(&projects, "/nonexistent/a/inner");
        // A layout that no longer parses does not hide its project.
        let broken = write_project(&projects, "/nonexistent/b");
        fs::write(
            broken.join(PROJECT_FILE),
            "root = \"/nonexistent/b\"\n[layout]\nfocused_group = \"x\"\n",
        )
        .unwrap();
        fs::create_dir_all(projects.join("stray")).unwrap();

        let mut roots: Vec<PathBuf> = list_in(&projects)
            .into_iter()
            .map(|p| p.project_path)
            .collect();
        roots.sort();
        assert_eq!(
            roots,
            ["/nonexistent/a", "/nonexistent/a/inner", "/nonexistent/b"].map(PathBuf::from)
        );
        let nested: Vec<PathBuf> = nested_in(&projects, Path::new("/nonexistent/a"))
            .into_iter()
            .map(|p| p.project_path)
            .collect();
        assert_eq!(nested, [PathBuf::from("/nonexistent/a/inner")]);
    }

    #[test]
    fn deleting_a_project_removes_its_directory_but_not_one_held() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join(PROJECTS_DIR);
        let dir = write_project(&projects, "/nonexistent/a");
        fs::create_dir_all(dir.join(BUFFERS_DIR)).unwrap();
        fs::write(dir.join(BUFFERS_DIR).join("1.txt"), "draft").unwrap();
        fs::write(dir.join(TRUST_FILE), "").unwrap();

        let claim = claim_in(&dir, Some("other")).unwrap();
        match delete_dir(&dir) {
            Err(DeleteError::Open(owner)) => assert_eq!(owner.instance.as_deref(), Some("other")),
            other => panic!("expected the project to be refused, got {other:?}"),
        }
        assert!(dir.join(PROJECT_FILE).exists());
        drop(claim);

        delete_dir(&dir).unwrap();
        assert!(!dir.exists());
        assert!(
            delete_dir(&dir).is_ok(),
            "deleting what is gone is no error"
        );
    }

    #[test]
    fn the_sweep_drops_only_directories_with_nothing_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join(PROJECTS_DIR);
        let project = write_project(&projects, "/nonexistent/a");
        let granted = projects.join("granted-000000000001");
        fs::create_dir_all(&granted).unwrap();
        fs::write(granted.join(TRUST_FILE), "").unwrap();
        // What a crash leaves of a project opened and never saved.
        let leftover = projects.join("leftover-000000000002");
        fs::create_dir_all(&leftover).unwrap();
        fs::write(leftover.join(LOCK_FILE), "").unwrap();
        fs::write(leftover.join(OWNER_FILE), "pid = 1\n").unwrap();
        let held = projects.join("held-000000000003");
        let _claim = claim_in(&held, None).unwrap();

        sweep_in(&projects);

        assert!(project.exists());
        assert!(granted.exists());
        assert!(!leftover.exists());
        assert!(held.exists(), "a project opened and not saved yet stays");
    }
}
