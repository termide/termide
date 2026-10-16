//! The breakpoints of every file, shared by the editors of this process and
//! saved in `breakpoints.toml` in termide's data directory.
//!
//! An editor holds the breakpoints of its file while it is open (they move
//! with its edits) and writes them back here whenever they change; a debug
//! launch reads them all from here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Breakpoint;

const FILE_NAME: &str = "breakpoints.toml";

/// Breakpoints by file. Lines are 0-based here and 1-based on disk.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Breakpoints {
    files: BTreeMap<PathBuf, BTreeSet<usize>>,
    /// Files changed since the last save; only these are written over what
    /// the file on disk holds, so two instances do not undo each other.
    touched: BTreeSet<PathBuf>,
    /// Bumped by every change, for a running debug session to follow.
    version: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct OnDisk {
    #[serde(default)]
    files: BTreeMap<String, Vec<usize>>,
}

impl Breakpoints {
    /// Lines with a breakpoint in `path`, 0-based.
    pub fn lines(&self, path: &Path) -> BTreeSet<usize> {
        self.files.get(&key(path)).cloned().unwrap_or_default()
    }

    /// Replace the breakpoints of `path`; no lines removes the file.
    pub fn set_lines(&mut self, path: &Path, lines: impl IntoIterator<Item = usize>) {
        let path = key(path);
        let lines: BTreeSet<usize> = lines.into_iter().collect();
        let old = self.files.get(&path);
        if old.map_or(lines.is_empty(), |old| *old == lines) {
            return;
        }
        if lines.is_empty() {
            self.files.remove(&path);
        } else {
            self.files.insert(path.clone(), lines);
        }
        self.touched.insert(path);
        self.version += 1;
    }

    /// Bumped by every change of the breakpoints.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Every breakpoint, by file and then by line.
    pub fn all(&self) -> Vec<Breakpoint> {
        self.files
            .iter()
            .flat_map(|(path, lines)| {
                lines.iter().map(move |l| Breakpoint {
                    path: path.clone(),
                    line: l + 1,
                })
            })
            .collect()
    }

    /// Number of breakpoints in all files.
    pub fn count(&self) -> usize {
        self.files.values().map(BTreeSet::len).sum()
    }

    /// Remove every breakpoint.
    pub fn clear(&mut self) {
        let paths: Vec<PathBuf> = self.files.keys().cloned().collect();
        self.touched.extend(paths);
        self.files.clear();
        self.version += 1;
    }

    /// Read the breakpoints saved in `file`; a missing file holds none.
    pub fn load_from(file: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("Failed to read {}", file.display())),
        };
        let disk: OnDisk =
            toml::from_str(&text).with_context(|| format!("Failed to parse {}", file.display()))?;
        let files = disk
            .files
            .into_iter()
            .filter_map(|(path, lines)| {
                let lines: BTreeSet<usize> = lines
                    .into_iter()
                    .filter(|&l| l > 0)
                    .map(|l| l - 1)
                    .collect();
                (!lines.is_empty()).then(|| (PathBuf::from(path), lines))
            })
            .collect();
        Ok(Self {
            files,
            touched: BTreeSet::new(),
            version: 0,
        })
    }

    /// Write the files changed since the last save into `file`, keeping
    /// what it holds for the others.
    pub fn save_to(&mut self, file: &Path) -> Result<()> {
        if self.touched.is_empty() {
            return Ok(());
        }
        let mut merged = Self::load_from(file).unwrap_or_else(|e| {
            log::warn!("Breakpoints: {e:#}; rewriting the file");
            Self::default()
        });
        for path in &self.touched {
            match self.files.get(path) {
                Some(lines) => {
                    merged.files.insert(path.clone(), lines.clone());
                }
                None => {
                    merged.files.remove(path);
                }
            }
        }
        let disk = OnDisk {
            files: merged
                .files
                .iter()
                .map(|(path, lines)| {
                    let lines = lines.iter().map(|l| l + 1).collect();
                    (path.to_string_lossy().into_owned(), lines)
                })
                .collect(),
        };
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create {}", dir.display()))?;
        }
        let text = toml::to_string_pretty(&disk).context("Failed to serialize breakpoints")?;
        std::fs::write(file, text)
            .with_context(|| format!("Failed to write {}", file.display()))?;
        self.touched.clear();
        Ok(())
    }
}

/// Breakpoints are keyed by absolute path, so the same file reached from
/// another directory is the same entry.
fn key(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn storage_file() -> Option<PathBuf> {
    termide_config::get_data_dir()
        .ok()
        .map(|d| d.join(FILE_NAME))
}

/// The breakpoints of this process, read from disk on first use.
pub fn store() -> MutexGuard<'static, Breakpoints> {
    static STORE: OnceLock<Mutex<Breakpoints>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let loaded = storage_file()
                .map(|file| {
                    Breakpoints::load_from(&file).unwrap_or_else(|e| {
                        log::warn!("Breakpoints: {e:#}");
                        Breakpoints::default()
                    })
                })
                .unwrap_or_default();
            Mutex::new(loaded)
        })
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Save the breakpoints changed since the last save.
pub fn flush() {
    let Some(file) = storage_file() else {
        return;
    };
    if let Err(e) = store().save_to(&file) {
        log::warn!("Breakpoints: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(b: &Breakpoints, path: &str) -> Vec<usize> {
        b.lines(Path::new(path)).into_iter().collect()
    }

    #[test]
    fn set_and_remove_lines() {
        let mut b = Breakpoints::default();
        b.set_lines(Path::new("/p/a.rs"), [4, 1]);
        assert_eq!(lines(&b, "/p/a.rs"), [1, 4]);
        let all: Vec<(PathBuf, usize)> = b.all().into_iter().map(|b| (b.path, b.line)).collect();
        assert_eq!(all, [("/p/a.rs".into(), 2), ("/p/a.rs".into(), 5)]);
        b.set_lines(Path::new("/p/a.rs"), []);
        assert_eq!(b.count(), 0);
    }

    #[test]
    fn saving_keeps_the_files_another_instance_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);

        let mut first = Breakpoints::default();
        first.set_lines(Path::new("/p/a.rs"), [0]);
        first.save_to(&file).unwrap();

        // Another instance, loaded before the first saved, changes only b.rs.
        let mut second = Breakpoints::default();
        second.set_lines(Path::new("/p/b.rs"), [9]);
        second.save_to(&file).unwrap();

        let on_disk = Breakpoints::load_from(&file).unwrap();
        assert_eq!(lines(&on_disk, "/p/a.rs"), [0]);
        assert_eq!(lines(&on_disk, "/p/b.rs"), [9]);
        assert!(std::fs::read_to_string(&file)
            .unwrap()
            .contains("\"/p/b.rs\" = [10]"));

        // Removing every breakpoint of a file removes it from disk too.
        second.set_lines(Path::new("/p/b.rs"), []);
        second.save_to(&file).unwrap();
        let on_disk = Breakpoints::load_from(&file).unwrap();
        assert!(lines(&on_disk, "/p/b.rs").is_empty());
        assert_eq!(lines(&on_disk, "/p/a.rs"), [0]);
    }

    #[test]
    fn a_missing_file_holds_none() {
        let dir = tempfile::tempdir().unwrap();
        let b = Breakpoints::load_from(&dir.path().join("none.toml")).unwrap();
        assert_eq!(b.count(), 0);
    }
}
