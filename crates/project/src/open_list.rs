//! The projects open together, kept for the Projects menu to reopen after a
//! restart.
//!
//! One file for every instance: the last one to change its set of open
//! projects wins. It is rewritten whenever that set changes, never only on
//! quit, so it survives a crash or a killed terminal.
//!
//! The current project is kept beside the set, so that `termide --restore`
//! reopens the run as it stood instead of adding the directory it happens to
//! have been started in.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::get_data_dir;

const OPEN_PROJECTS_FILE: &str = "open_projects.toml";

#[derive(Debug, Default, Serialize, Deserialize)]
struct OpenProjectsFile {
    #[serde(default)]
    roots: Vec<PathBuf>,
    /// The project that was current when the set was saved. Absent only in a
    /// file written before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<PathBuf>,
}

/// What the last run left open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedOpenProjects {
    /// The open projects, in the order the menus listed them.
    pub roots: Vec<PathBuf>,
    /// Which of them was current, `None` when the file does not say.
    pub current: Option<PathBuf>,
}

impl SavedOpenProjects {
    /// Nothing saved.
    fn empty() -> Self {
        Self {
            roots: Vec::new(),
            current: None,
        }
    }

    /// The directory `termide --restore` starts in, or `None` to stay in the
    /// working directory.
    ///
    /// Stays when the saved set has the working directory open: launching
    /// from a project of the set says you want to be there. Otherwise it is
    /// the project the last run was in, so that launching from an unrelated
    /// directory reopens that run instead of adding this directory to it.
    pub fn start_dir(&self, cwd: &Path) -> Option<PathBuf> {
        self.start_dir_by(cwd, layout_saved_at)
    }

    /// `start_dir` with the layout timestamps read through `saved_at`, so the
    /// fallback of a file that names no current project is testable.
    fn start_dir_by(
        &self,
        cwd: &Path,
        saved_at: impl Fn(&Path) -> Option<std::time::SystemTime>,
    ) -> Option<PathBuf> {
        if self.roots.iter().any(|root| same_root(root, cwd)) {
            return None;
        }
        let root = match &self.current {
            Some(current) => current.clone(),
            // A file written before the current project was kept: the root
            // whose layout was saved last is where the last run most likely
            // was, and it is what the Projects menu sorts by.
            None => self
                .roots
                .iter()
                .map(|root| (saved_at(root), root.clone()))
                .max_by_key(|(saved, _)| *saved)
                .map(|(_, root)| root)?,
        };
        root.is_dir().then_some(root)
    }
}

/// When the layout of `root` was last saved, unknown as `None`.
fn layout_saved_at(root: &Path) -> Option<std::time::SystemTime> {
    crate::ProjectLayout::get_project_path(root)
        .ok()
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
}

/// Whether both paths name the same directory, with symlinks resolved so one
/// project reached by two paths is recognised as one.
fn same_root(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The projects open together in the last run. Empty when none were saved or
/// the file cannot be read.
pub fn load_open_projects() -> SavedOpenProjects {
    match get_data_dir() {
        Ok(dir) => load_from(&dir.join(OPEN_PROJECTS_FILE)),
        Err(_) => SavedOpenProjects::empty(),
    }
}

/// Save `projects` as the set open together and the current one among them.
pub fn save_open_projects(projects: &SavedOpenProjects) -> Result<()> {
    save_to(&get_data_dir()?.join(OPEN_PROJECTS_FILE), projects)
}

fn load_from(path: &Path) -> SavedOpenProjects {
    let Ok(contents) = fs::read_to_string(path) else {
        return SavedOpenProjects::empty();
    };
    match toml::from_str::<OpenProjectsFile>(&contents) {
        Ok(file) => SavedOpenProjects {
            roots: file.roots,
            current: file.current,
        },
        Err(e) => {
            log::warn!("Ignoring unreadable {}: {}", path.display(), e);
            SavedOpenProjects::empty()
        }
    }
}

/// Write through a temporary file and a rename, so a crash midway leaves
/// the previous list rather than a truncated one. The temporary name carries
/// the process id: two instances may write at once.
fn save_to(path: &Path, projects: &SavedOpenProjects) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    }
    let file = OpenProjectsFile {
        roots: projects.roots.clone(),
        current: projects.current.clone(),
    };
    let contents = toml::to_string_pretty(&file).context("Failed to serialize open projects")?;
    let tmp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    fs::write(&tmp, contents).with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        let _ = fs::remove_file(&tmp);
        format!("Failed to replace {}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_roots_load_back_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(OPEN_PROJECTS_FILE);
        let roots = vec![PathBuf::from("/p/b"), PathBuf::from("/p/a")];
        save_to(
            &path,
            &SavedOpenProjects {
                roots: roots.clone(),
                current: Some(PathBuf::from("/p/a")),
            },
        )
        .unwrap();
        let saved = load_from(&path);
        assert_eq!(saved.roots, roots);
        assert_eq!(saved.current, Some(PathBuf::from("/p/a")));

        save_to(
            &path,
            &SavedOpenProjects {
                roots: roots[..1].to_vec(),
                current: None,
            },
        )
        .unwrap();
        assert_eq!(load_from(&path).roots, roots[..1]);
        let leftovers = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file is left behind");
    }

    #[test]
    fn a_missing_or_broken_file_reads_as_no_projects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(OPEN_PROJECTS_FILE);
        assert_eq!(load_from(&path), SavedOpenProjects::empty());
        fs::write(&path, "roots = 42").unwrap();
        assert_eq!(load_from(&path), SavedOpenProjects::empty());
    }

    /// A file written before the current project was kept still reads back,
    /// without one.
    #[test]
    fn a_file_without_a_current_project_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(OPEN_PROJECTS_FILE);
        fs::write(&path, "roots = [\"/p/a\"]\n").unwrap();
        let saved = load_from(&path);
        assert_eq!(saved.roots, vec![PathBuf::from("/p/a")]);
        assert_eq!(saved.current, None);
    }

    #[test]
    fn restoring_starts_in_the_project_of_the_last_run() {
        let dir = tempfile::tempdir().unwrap();
        let here = dir.path().join("here");
        let there = dir.path().join("there");
        let elsewhere = dir.path().join("elsewhere");
        for d in [&here, &there] {
            fs::create_dir_all(d).unwrap();
        }

        let saved = SavedOpenProjects {
            roots: vec![here.clone(), there.clone()],
            current: Some(there.clone()),
        };
        // Started from a project of the set: stay there.
        assert_eq!(saved.start_dir(&here), None);
        // Reached through a symlink, it is still the same project.
        assert_eq!(saved.start_dir(&canonical(&here)), None);
        // Started from an unrelated directory: go back to the last one.
        assert_eq!(saved.start_dir(&elsewhere), Some(there.clone()));

        // A current project that no longer exists names nowhere to start.
        let gone = SavedOpenProjects {
            roots: vec![here.clone()],
            current: Some(elsewhere.clone()),
        };
        assert_eq!(gone.start_dir(&there), None);
    }

    /// A file written before the current project was kept falls back to the
    /// root whose layout was saved last.
    #[test]
    fn a_file_without_a_current_project_falls_back_to_the_layout_saved_last() {
        let dir = tempfile::tempdir().unwrap();
        let older = dir.path().join("older");
        let newer = dir.path().join("newer");
        fs::create_dir_all(&older).unwrap();
        fs::create_dir_all(&newer).unwrap();

        let saved_at = |root: &Path| {
            Some(
                std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(if root == newer { 200 } else { 100 }),
            )
        };
        let saved = SavedOpenProjects {
            roots: vec![older.clone(), newer.clone()],
            current: None,
        };
        assert_eq!(
            saved.start_dir_by(Path::new("/elsewhere"), saved_at),
            Some(newer)
        );

        // A root that is gone leaves the launch where it is.
        let gone = SavedOpenProjects {
            roots: vec![dir.path().join("deleted")],
            current: None,
        };
        assert_eq!(gone.start_dir_by(Path::new("/elsewhere"), |_| None), None);
    }
}
