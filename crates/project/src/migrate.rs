//! Bringing what older termides stored into today's project store.
//!
//! Two layouts came before it. The oldest kept saved layouts in
//! `<data>/sessions/`; then they moved to `<data>/projects/`, still in a
//! tree mirroring each project's path, with the grants in a tree of their
//! own under `<data>/trust/` and the locks under `<data>/locks/`. Each
//! project now has one directory, side by side with the others (see
//! [`crate::store`]).
//!
//! Runs at every start, before the logger is up, so the outcome is returned
//! for the caller to log: an older termide may have written the old places
//! again since the last one.

use std::fs::{self, File, TryLockError};
use std::path::{Path, PathBuf};

use crate::ownership::{LOCK_FILE, OWNER_FILE};
use crate::store::{project_dir_in, BUFFERS_DIR, LOGS_DIR, PROJECTS_DIR, PROJECT_FILE, TRUST_FILE};
use crate::{get_data_dir, ProjectFileOut, ProjectLayout};

type Notes = Vec<(log::Level, String)>;

/// The directory of the oldest layouts, before the projects rename.
const LEGACY_PROJECTS_DIR: &str = "sessions";
/// The tree of grants, by project path.
const LEGACY_TRUST_DIR: &str = "trust";
/// The tree of locks, by project path.
const LEGACY_LOCKS_DIR: &str = "locks";
/// The layout file of the tree.
const LEGACY_LAYOUT: &str = "session.toml";
/// The grants of the tree: "allow always" rules and approved launches.
const LEGACY_PERMISSIONS: &str = "permissions.toml";
const LEGACY_APPROVED: &str = "approved.toml";

/// Bring the layouts, buffers, logs, grants and locks of older termides
/// into the project store.
pub fn migrate_legacy_layouts() -> Notes {
    match get_data_dir() {
        Ok(data_dir) => {
            let mut notes = migrate_legacy_dir(&data_dir);
            notes.extend(migrate_tree(&data_dir));
            notes
        }
        Err(_) => Vec::new(),
    }
}

/// Move `<data>/sessions` to `<data>/projects`, or fold it into `projects`
/// when both exist. Failures are reported, never fatal: a fresh directory is
/// created instead and only the old layouts are lost.
fn migrate_legacy_dir(data_dir: &Path) -> Notes {
    let legacy = data_dir.join(LEGACY_PROJECTS_DIR);
    let current = data_dir.join(PROJECTS_DIR);
    if !legacy.is_dir() {
        return Vec::new();
    }
    let (from, to) = (legacy.display(), current.display());
    if !current.exists() {
        return vec![match fs::rename(&legacy, &current) {
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
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if !to.exists() {
            fs::rename(&from, &to)?;
        } else if from.is_dir() && to.is_dir() {
            merge_move(&from, &to)?;
        } else if from.is_file() && to.is_file() {
            if modified(&from) > modified(&to) {
                fs::rename(&from, &to)?;
            } else {
                fs::remove_file(&from)?;
            }
        }
    }
    // Only an empty directory goes; anything unexpected stays for the user.
    let _ = fs::remove_dir(src);
    Ok(())
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Whether `name` is that of a project directory of the store,
/// `<name>-<12 hex digits>`, rather than the first component of a path in
/// the old tree.
fn is_store_name(name: &str) -> bool {
    name.rsplit_once('-')
        .is_some_and(|(_, hash)| hash.len() == 12 && hash.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The root of the project a directory of an old tree was kept for: the
/// path it mirrors. The tree kept no drive prefix on Windows, so the path
/// is read on the current drive, as the old code read it.
fn root_of(tree: &Path, dir: &Path) -> Option<PathBuf> {
    let rel = dir.strip_prefix(tree).ok()?;
    let root = PathBuf::from(std::path::MAIN_SEPARATOR_STR).join(rel);
    Some(dunce::canonicalize(&root).unwrap_or(root))
}

/// The directories of the tree under `tree` that hold one of `files` (or,
/// for `files` empty, any file), its top-level store directories left out.
fn tree_dirs(tree: &Path, holds: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    fn walk(dir: &Path, holds: &dyn Fn(&Path) -> bool, found: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        if holds(dir) {
            found.push(dir.to_path_buf());
        }
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&entry.path(), holds, found);
            }
        }
    }
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(tree) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if entry.file_type().is_ok_and(|t| t.is_dir()) && !is_store_name(&name.to_string_lossy()) {
            walk(&entry.path(), holds, &mut found);
        }
    }
    found
}

/// Remove the directories under `tree` left empty, deepest first; `tree`
/// itself too when `with_root`.
fn prune_empty(tree: &Path, with_root: bool) {
    fn prune(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    prune(&entry.path());
                }
            }
        }
        let _ = fs::remove_dir(dir);
    }
    let Ok(entries) = fs::read_dir(tree) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if entry.file_type().is_ok_and(|t| t.is_dir()) && !is_store_name(&name.to_string_lossy()) {
            prune(&entry.path());
        }
    }
    if with_root {
        let _ = fs::remove_dir(tree);
    }
}

/// Bring the tree of `projects/`, `trust/` and `locks/` into the store.
fn migrate_tree(data_dir: &Path) -> Notes {
    let mut notes = Vec::new();
    let projects = data_dir.join(PROJECTS_DIR);
    let layouts = tree_dirs(&projects, &|dir| {
        fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        })
    });
    let mut layouts = layouts;
    // The project at `/` kept its files at the top of the tree.
    if projects.join(LEGACY_LAYOUT).is_file() {
        layouts.insert(0, projects.clone());
    }
    let mut moved = 0;
    for dir in &layouts {
        let Some(root) = root_of(&projects, dir) else {
            continue;
        };
        match migrate_project(dir, &project_dir_in(data_dir, &root), &root) {
            Ok(()) => moved += 1,
            Err(e) => notes.push((
                log::Level::Warn,
                format!(
                    "could not bring {} into the project store: {e}",
                    dir.display()
                ),
            )),
        }
    }
    prune_empty(&projects, false);
    if moved > 0 {
        notes.push((
            log::Level::Info,
            format!(
                "moved {moved} saved project(s) into one directory each under {}",
                projects.display()
            ),
        ));
    }

    let trust = data_dir.join(LEGACY_TRUST_DIR);
    let grants = tree_dirs(&trust, &|dir| {
        dir.join(LEGACY_PERMISSIONS).is_file() || dir.join(LEGACY_APPROVED).is_file()
    });
    for dir in &grants {
        let Some(root) = root_of(&trust, dir) else {
            continue;
        };
        if let Err(e) = migrate_grants(dir, &project_dir_in(data_dir, &root)) {
            notes.push((
                log::Level::Warn,
                format!(
                    "could not bring the grants in {} into the project store: {e}",
                    dir.display()
                ),
            ));
        }
    }
    prune_empty(&trust, true);

    release_old_locks(&data_dir.join(LEGACY_LOCKS_DIR));
    notes
}

/// Move what the old directory `old` kept of the project at `root` into its
/// store directory `new`: the layout into `project.toml`, buffers into
/// `buffers/`, run logs into `logs/`. A layout the store already has saved
/// later wins. Subdirectories, the projects inside this one, stay.
fn migrate_project(old: &Path, new: &Path, root: &Path) -> std::io::Result<()> {
    fs::create_dir_all(new)?;
    let layout_file = old.join(LEGACY_LAYOUT);
    if layout_file.is_file() {
        let current = new.join(PROJECT_FILE);
        if current.exists() && modified(&current) >= modified(&layout_file) {
            fs::remove_file(&layout_file)?;
        } else {
            let text = fs::read_to_string(&layout_file)?;
            match toml::from_str::<ProjectLayout>(&text) {
                Ok(layout) => {
                    let text = toml::to_string_pretty(&ProjectFileOut {
                        root,
                        layout: &layout,
                    })
                    .map_err(std::io::Error::other)?;
                    termide_fsutil::write_atomic(&current, text.as_bytes())?;
                    fs::remove_file(&layout_file)?;
                }
                // Kept as it was, beside a project file naming the root, for
                // the user to look at.
                Err(_) => {
                    if !current.exists() {
                        let text =
                            toml::to_string(&RootFile { root }).map_err(std::io::Error::other)?;
                        fs::write(&current, text)?;
                    }
                    fs::rename(&layout_file, new.join("project.toml.broken-migrated"))?;
                }
            }
        }
    }
    for entry in fs::read_dir(old)?.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let target = if name.starts_with("unsaved-") && name.ends_with(".txt") {
            new.join(BUFFERS_DIR).join(&name)
        } else if let Some(stamp) = name
            .strip_prefix("session-")
            .filter(|n| n.ends_with(".log"))
        {
            new.join(LOGS_DIR).join(stamp)
        } else if let Some(stamp) = name.strip_prefix("session.toml.broken-") {
            new.join(format!("project.toml.broken-{stamp}"))
        } else {
            new.join(&name)
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(entry.path(), target)?;
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct RootFile<'a> {
    root: &'a Path,
}

/// Fold the grants kept in the old directory `old` into the `trust.toml` of
/// the store directory `new`: the rules as they were (`[ai.permissions]`),
/// the approved launches under `[approved]`. What `trust.toml` holds already
/// wins.
fn migrate_grants(old: &Path, new: &Path) -> std::io::Result<()> {
    let read = |name: &str| -> std::io::Result<Option<toml::Table>> {
        match fs::read_to_string(old.join(name)) {
            Ok(text) => toml::from_str(&text)
                .map(Some)
                .map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    };
    let mut merged = toml::Table::new();
    if let Some(rules) = read(LEGACY_PERMISSIONS)? {
        merged = rules;
    }
    if let Some(approved) = read(LEGACY_APPROVED)? {
        merged.insert("approved".into(), toml::Value::Table(approved));
    }
    fs::create_dir_all(new)?;
    let path = new.join(TRUST_FILE);
    if let Ok(text) = fs::read_to_string(&path) {
        match toml::from_str::<toml::Table>(&text) {
            Ok(current) => merge_into(&mut merged, current),
            // Kept for the user to mend, as a broken layout is, rather than
            // overwritten by the migrated grants alone.
            Err(e) => {
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
                let mut aside = path.as_os_str().to_owned();
                aside.push(format!(".broken-{stamp}"));
                let aside = PathBuf::from(aside);
                fs::rename(&path, &aside)?;
                log::warn!(
                    "{} did not parse ({}); it was kept as {}",
                    path.display(),
                    e.message().trim(),
                    aside.display()
                );
            }
        }
    }
    let text = toml::to_string(&merged).map_err(std::io::Error::other)?;
    termide_fsutil::write_atomic(&path, text.as_bytes())?;
    let _ = fs::remove_file(old.join(LEGACY_PERMISSIONS));
    let _ = fs::remove_file(old.join(LEGACY_APPROVED));
    Ok(())
}

/// Lay `over` onto `base`, table by table, `over` winning.
fn merge_into(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(over)) => merge_into(base, over),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Remove the lock files of the old tree no instance holds: taken first,
/// so one an older termide still has open stays with it.
fn release_old_locks(locks: &Path) {
    let held = tree_dirs(locks, &|dir| dir.join(LOCK_FILE).is_file());
    for dir in held {
        let Ok(file) = File::options()
            .read(true)
            .write(true)
            .open(dir.join(LOCK_FILE))
        else {
            continue;
        };
        match file.try_lock() {
            Ok(()) => {
                let _ = fs::remove_file(dir.join(OWNER_FILE));
                let _ = fs::remove_file(dir.join(LOCK_FILE));
            }
            Err(TryLockError::WouldBlock | TryLockError::Error(_)) => {}
        }
    }
    prune_empty(locks, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{list_in, project_id};

    fn write(base: &Path, rel: &str, body: &str) {
        let path = base.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// A termide older than the rename keeps writing `sessions/` after the
    /// first move; what it leaves there is folded into `projects/`, newer
    /// files winning, and the empty shell disappears.
    #[test]
    fn leftover_legacy_layouts_are_folded_into_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join(LEGACY_PROJECTS_DIR);
        let current = tmp.path().join(PROJECTS_DIR);
        write(&legacy, "home/u/only-legacy/session.toml", "legacy only");
        write(&legacy, "home/u/both/session.toml", "legacy newer");
        write(&current, "home/u/both/session.toml", "current older");
        write(&current, "home/u/only-current/session.toml", "current only");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        File::open(current.join("home/u/both/session.toml"))
            .unwrap()
            .set_modified(old)
            .unwrap();

        let notes = migrate_legacy_dir(tmp.path());

        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, log::Level::Info, "{}", notes[0].1);
        assert!(!legacy.exists());
        let read = |rel: &str| fs::read_to_string(current.join(rel)).unwrap();
        assert_eq!(read("home/u/only-legacy/session.toml"), "legacy only");
        assert_eq!(read("home/u/both/session.toml"), "legacy newer");
        assert_eq!(read("home/u/only-current/session.toml"), "current only");

        // Idempotent: nothing left to do.
        assert!(migrate_legacy_dir(tmp.path()).is_empty());
    }

    /// The first launch after the upgrade finds only `sessions/` and moves
    /// the whole tree, reporting it for the journal.
    #[test]
    fn legacy_layouts_move_to_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join(LEGACY_PROJECTS_DIR);
        let current = tmp.path().join(PROJECTS_DIR);
        write(&legacy, "home/u/proj/session.toml", "layout");

        let notes = migrate_legacy_dir(tmp.path());

        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, log::Level::Info, "{}", notes[0].1);
        assert!(!legacy.exists());
        assert!(current.join("home/u/proj/session.toml").is_file());
        assert!(migrate_legacy_dir(tmp.path()).is_empty());
    }

    const LAYOUT: &str = "focused_group = 0\npanel_groups = []\n";

    /// The tree of layouts, grants and locks becomes one directory per
    /// project: the layout under `[layout]` beside the root, buffers and
    /// logs in their directories, both grant files in `trust.toml`; nested
    /// projects get directories of their own, and the old trees go.
    #[test]
    fn the_tree_becomes_one_directory_per_project() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let projects = data.join(PROJECTS_DIR);
        write(&projects, "nonexistent/a/session.toml", LAYOUT);
        write(&projects, "nonexistent/a/unsaved-1.txt", "draft");
        write(
            &projects,
            "nonexistent/a/session-20261112-120008-030.log",
            "log",
        );
        write(&projects, "nonexistent/a/session.toml.broken-1", "bad");
        write(&projects, "nonexistent/a/inner/session.toml", LAYOUT);
        write(&projects, "nonexistent/b/session.toml", "[[[ not toml");
        write(
            &data.join(LEGACY_TRUST_DIR),
            "nonexistent/a/permissions.toml",
            "[ai.permissions.shell]\nallow = [\"ls\"]\n",
        );
        write(
            &data.join(LEGACY_TRUST_DIR),
            "nonexistent/a/approved.toml",
            "[mcp]\nx = \"spec\"\n",
        );
        write(
            &data.join(LEGACY_TRUST_DIR),
            "nonexistent/c/approved.toml",
            "[hooks]\ny = \"spec\"\n",
        );
        write(&data.join(LEGACY_LOCKS_DIR), "nonexistent/a/owner.lock", "");
        write(
            &data.join(LEGACY_LOCKS_DIR),
            "nonexistent/a/owner.toml",
            "pid = 1\n",
        );

        let notes = migrate_tree(data);
        assert!(
            notes.iter().all(|(level, _)| *level == log::Level::Info),
            "{notes:?}"
        );

        let a = projects.join(project_id(Path::new("/nonexistent/a")));
        let file: toml::Table =
            toml::from_str(&fs::read_to_string(a.join(PROJECT_FILE)).unwrap()).unwrap();
        assert_eq!(file["root"].as_str(), Some("/nonexistent/a"));
        assert!(file["layout"].get("focused_group").is_some());
        assert_eq!(
            fs::read_to_string(a.join("buffers/unsaved-1.txt")).unwrap(),
            "draft"
        );
        assert!(a.join("logs/20261112-120008-030.log").is_file());
        assert!(a.join("project.toml.broken-1").is_file());
        let trust: toml::Table =
            toml::from_str(&fs::read_to_string(a.join(TRUST_FILE)).unwrap()).unwrap();
        assert_eq!(
            trust["ai"]["permissions"]["shell"]["allow"][0].as_str(),
            Some("ls")
        );
        assert_eq!(trust["approved"]["mcp"]["x"].as_str(), Some("spec"));

        let b = projects.join(project_id(Path::new("/nonexistent/b")));
        assert!(b.join("project.toml.broken-migrated").is_file());
        let c = projects.join(project_id(Path::new("/nonexistent/c")));
        assert!(c.join(TRUST_FILE).is_file());

        let mut roots: Vec<PathBuf> = list_in(&projects)
            .into_iter()
            .map(|p| p.project_path)
            .collect();
        roots.sort();
        assert_eq!(
            roots,
            ["/nonexistent/a", "/nonexistent/a/inner", "/nonexistent/b"].map(PathBuf::from)
        );
        assert!(!projects.join("nonexistent").exists());
        assert!(!data.join(LEGACY_TRUST_DIR).exists());
        assert!(!data.join(LEGACY_LOCKS_DIR).exists());

        // Nothing left to do, and the store's own directories are not taken
        // for the old tree.
        assert!(migrate_tree(data).is_empty());
        assert!(a.join("buffers/unsaved-1.txt").is_file());
    }

    /// A `trust.toml` that does not parse is set aside, not overwritten by
    /// the grants migrated into its place.
    #[test]
    fn a_broken_trust_file_is_kept_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old");
        let new = tmp.path().join("new");
        write(&old, "approved.toml", "[mcp]\nx = \"spec\"\n");
        write(&new, TRUST_FILE, "[[[ hand-edited, broken");

        migrate_grants(&old, &new).unwrap();

        let trust: toml::Table =
            toml::from_str(&fs::read_to_string(new.join(TRUST_FILE)).unwrap()).unwrap();
        assert_eq!(trust["approved"]["mcp"]["x"].as_str(), Some("spec"));
        let aside: Vec<_> = fs::read_dir(&new)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("trust.toml.broken-"))
            .collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        assert_eq!(
            fs::read_to_string(new.join(&aside[0])).unwrap(),
            "[[[ hand-edited, broken"
        );
    }

    /// An older termide still running writes its layout in the tree again;
    /// the next start takes it only when it is newer than the store's.
    #[test]
    fn a_layout_written_again_in_the_tree_wins_only_when_newer() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let projects = data.join(PROJECTS_DIR);
        let a = projects.join(project_id(Path::new("/nonexistent/a")));
        write(
            &a,
            PROJECT_FILE,
            "root = \"/nonexistent/a\"\n[layout]\nfocused_group = 7\npanel_groups = []\n",
        );
        write(&projects, "nonexistent/a/session.toml", LAYOUT);
        File::open(projects.join("nonexistent/a/session.toml"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(600))
            .unwrap();

        migrate_tree(data);
        let text = fs::read_to_string(a.join(PROJECT_FILE)).unwrap();
        assert!(text.contains("focused_group = 7"), "{text}");

        write(&projects, "nonexistent/a/session.toml", LAYOUT);
        File::open(a.join(PROJECT_FILE))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(600))
            .unwrap();
        migrate_tree(data);
        let text = fs::read_to_string(a.join(PROJECT_FILE)).unwrap();
        assert!(text.contains("focused_group = 0"), "{text}");
    }

    /// A lock an older termide still holds stays with it.
    #[test]
    fn an_old_lock_still_held_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let locks = tmp.path().join(LEGACY_LOCKS_DIR);
        write(&locks, "nonexistent/a/owner.lock", "");
        let held = File::options()
            .read(true)
            .write(true)
            .open(locks.join("nonexistent/a/owner.lock"))
            .unwrap();
        held.try_lock().unwrap();
        release_old_locks(&locks);
        assert!(locks.join("nonexistent/a/owner.lock").exists());
        drop(held);
        release_old_locks(&locks);
        assert!(!locks.exists());
    }
}
