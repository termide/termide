//! The projects open together, kept for the Projects menu to reopen after a
//! restart.
//!
//! One file for every instance: the last one to change its set of open
//! projects wins. It is rewritten whenever that set changes, never only on
//! quit, so it survives a crash or a killed terminal.

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
}

/// The project roots saved last, in the order they were opened. Empty when
/// none were saved or the file cannot be read.
pub fn load_open_projects() -> Vec<PathBuf> {
    match get_data_dir() {
        Ok(dir) => load_from(&dir.join(OPEN_PROJECTS_FILE)),
        Err(_) => Vec::new(),
    }
}

/// Save `roots` as the projects open together.
pub fn save_open_projects(roots: &[PathBuf]) -> Result<()> {
    save_to(&get_data_dir()?.join(OPEN_PROJECTS_FILE), roots)
}

fn load_from(path: &Path) -> Vec<PathBuf> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    match toml::from_str::<OpenProjectsFile>(&contents) {
        Ok(file) => file.roots,
        Err(e) => {
            log::warn!("Ignoring unreadable {}: {}", path.display(), e);
            Vec::new()
        }
    }
}

/// Write through a temporary file and a rename, so a crash midway leaves
/// the previous list rather than a truncated one. The temporary name carries
/// the process id: two instances may write at once.
fn save_to(path: &Path, roots: &[PathBuf]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    }
    let file = OpenProjectsFile {
        roots: roots.to_vec(),
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
        save_to(&path, &roots).unwrap();
        assert_eq!(load_from(&path), roots);

        save_to(&path, &roots[..1]).unwrap();
        assert_eq!(load_from(&path), roots[..1]);
        let leftovers = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file is left behind");
    }

    #[test]
    fn a_missing_or_broken_file_reads_as_no_projects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(OPEN_PROJECTS_FILE);
        assert!(load_from(&path).is_empty());
        fs::write(&path, "roots = 42").unwrap();
        assert!(load_from(&path).is_empty());
    }
}
