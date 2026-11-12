//! Carrying a project's session logs along when the project's directory is
//! moved or copied.
//!
//! The logs of a project are filed under its path (see
//! `termide_project::project_key`), those of directories inside it nested
//! under that; all of them follow the project. A log names the directory it
//! worked in in its header and its checkpoints name the files they keep by
//! absolute path, so both are rewritten for the new place. A copy is a new
//! project: its logs get ids of their own, so that no two logs answer to one
//! id.

use std::collections::HashMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::session::new_id;

/// The directory a session's checkpoints live under, beside its log.
const CHECKPOINTS_DIR: &str = "checkpoints";

/// The two places of a project whose session logs are carried along.
#[derive(Debug, Clone, Copy)]
pub struct SessionsRelocation<'a> {
    /// The session directory filed under the project's old path.
    pub from_dir: &'a Path,
    /// The session directory filed under the new path.
    pub to_dir: &'a Path,
    /// The project's old root.
    pub from_root: &'a Path,
    /// The project's new root.
    pub to_root: &'a Path,
    /// Copy, giving the copied logs new ids, rather than move.
    pub copy: bool,
}

/// Move or copy the logs as `relocation` says, and return where each log
/// went: its old path mapped to its new one, for saved layouts to follow.
/// What `to_dir` held already stays, but for a log of the same name.
///
/// # Errors
///
/// When the logs cannot be moved or copied; a log whose header cannot be
/// rewritten is logged and left as it is.
pub fn relocate_sessions(relocation: SessionsRelocation) -> io::Result<HashMap<PathBuf, PathBuf>> {
    let SessionsRelocation {
        from_dir,
        to_dir,
        copy,
        ..
    } = relocation;
    let is_lock = |path: &Path| path.extension().is_some_and(|ext| ext == "lock");
    let placed = termide_fsutil::transfer_tree(from_dir, to_dir, copy, &is_lock)?;

    // The logs first: a copy's new ids decide where its checkpoints go.
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut logs: Vec<(PathBuf, PathBuf)> = Vec::new();
    for path in &placed {
        let Ok(rel) = path.strip_prefix(to_dir) else {
            continue;
        };
        if in_checkpoints(rel) || path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let mut target = path.clone();
        if copy {
            if let Some((old, renamed)) = renamed_log(path) {
                let new = renamed.1;
                ids.insert(old, new);
                target = renamed.0;
            }
        }
        logs.push((from_dir.join(rel), target.clone()));
        if target != *path {
            std::fs::rename(path, &target)?;
        }
    }
    for (_, log) in &logs {
        if let Err(e) = rewrite_header(log, relocation, &ids) {
            log::warn!("Could not rewrite the header of {}: {e}", log.display());
        }
    }

    // A copy's checkpoints follow their session's new id.
    if copy {
        rename_checkpoint_dirs(to_dir, &ids)?;
    }
    for manifest in manifests(to_dir) {
        if let Err(e) = rewrite_manifest(&manifest, relocation, &ids) {
            log::warn!("Could not rewrite {}: {e}", manifest.display());
        }
    }
    Ok(logs.into_iter().collect())
}

/// Whether `rel`, relative to a session directory, lies among checkpoints.
fn in_checkpoints(rel: &Path) -> bool {
    rel.components()
        .any(|c| matches!(c, Component::Normal(name) if name == CHECKPOINTS_DIR))
}

/// For a log named `<time>_<id>.jsonl`: its id, and the path and id it
/// takes as a copy.
fn renamed_log(path: &Path) -> Option<(String, (PathBuf, String))> {
    let stem = path.file_stem()?.to_str()?;
    let (time, id) = stem.rsplit_once('_')?;
    let new = new_id();
    let target = path.with_file_name(format!("{time}_{new}.jsonl"));
    Some((id.to_string(), (target, new)))
}

/// Where `path` lies after the relocation: inside the old session
/// directory, in the new one, a copied session's checkpoints under its new
/// id; inside the old root, in the new one; elsewhere, where it was.
fn relocated(
    path: &Path,
    relocation: SessionsRelocation,
    ids: &HashMap<String, String>,
) -> PathBuf {
    let from_checkpoints = relocation.from_dir.join(CHECKPOINTS_DIR);
    if let Ok(rest) = path.strip_prefix(&from_checkpoints) {
        let mut parts = rest.components();
        if let Some(Component::Normal(id)) = parts.next() {
            let id = id.to_string_lossy();
            let id = ids.get(id.as_ref()).map_or(id.as_ref(), String::as_str);
            return relocation
                .to_dir
                .join(CHECKPOINTS_DIR)
                .join(id)
                .join(parts.as_path());
        }
    }
    termide_fsutil::rebase(path, relocation.from_dir, relocation.to_dir)
        .or_else(|| termide_fsutil::rebase(path, relocation.from_root, relocation.to_root))
        .unwrap_or_else(|| path.to_path_buf())
}

/// Rewrite the first line of the log at `path`: the working directory, and
/// for a copy the id and the id of the session that delegated it. Fields
/// this build does not know are kept as they are.
fn rewrite_header(
    path: &Path,
    relocation: SessionsRelocation,
    ids: &HashMap<String, String>,
) -> io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let (first, rest) = text.split_once('\n').unwrap_or((&text, ""));
    let mut header: Value = serde_json::from_str(first)?;
    if let Some(cwd) = header.get_mut("cwd") {
        if let Some(old) = cwd.as_str() {
            *cwd = Value::from(relocated(Path::new(old), relocation, ids).to_string_lossy());
        }
    }
    for pointer in ["/id", "/parent/session"] {
        if let Some(id) = header.pointer_mut(pointer) {
            if let Some(new) = id.as_str().and_then(|old| ids.get(old)) {
                *id = Value::from(new.as_str());
            }
        }
    }
    let mut out = serde_json::to_string(&header)?;
    out.push('\n');
    out.push_str(rest);
    termide_fsutil::write_atomic(path, out.as_bytes())
}

/// Rename the checkpoint directories of copied sessions, in `dir` and the
/// session directories nested in it, after their sessions' new ids.
fn rename_checkpoint_dirs(dir: &Path, ids: &HashMap<String, String>) -> io::Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if entry.file_name() == CHECKPOINTS_DIR {
            for session in std::fs::read_dir(&path)?.flatten() {
                let name = session.file_name();
                if let Some(new) = ids.get(name.to_string_lossy().as_ref()) {
                    std::fs::rename(session.path(), path.join(new))?;
                }
            }
        } else {
            rename_checkpoint_dirs(&path, ids)?;
        }
    }
    Ok(())
}

/// Every checkpoint manifest under `dir`.
fn manifests(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if entry.file_name() == "manifest.json"
                && path.components().any(|c| c.as_os_str() == CHECKPOINTS_DIR)
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, &mut found);
    found
}

/// Point a checkpoint manifest's files and copies at their new places. Its
/// copies sit in the session directory, so they move with the logs.
fn rewrite_manifest(
    path: &Path,
    relocation: SessionsRelocation,
    ids: &HashMap<String, String>,
) -> io::Result<()> {
    let mut manifest: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let Some(files) = manifest.get_mut("files").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for file in files {
        for key in ["path", "copy"] {
            if let Some(value) = file.get_mut(key) {
                if let Some(old) = value.as_str() {
                    *value =
                        Value::from(relocated(Path::new(old), relocation, ids).to_string_lossy());
                }
            }
        }
    }
    let text = serde_json::to_string_pretty(&manifest)?;
    termide_fsutil::write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    struct Fixture {
        _tmp: tempfile::TempDir,
        from_dir: PathBuf,
        to_dir: PathBuf,
        from_root: PathBuf,
        to_root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().to_path_buf();
            Self {
                from_dir: base.join("sessions/a/proj"),
                to_dir: base.join("sessions/b/new"),
                from_root: PathBuf::from("/a/proj"),
                to_root: PathBuf::from("/b/new"),
                _tmp: tmp,
            }
        }

        fn relocation(&self, copy: bool) -> SessionsRelocation<'_> {
            SessionsRelocation {
                from_dir: &self.from_dir,
                to_dir: &self.to_dir,
                from_root: &self.from_root,
                to_root: &self.to_root,
                copy,
            }
        }

        /// A log working in `cwd`, filed in `dir`, with one checkpoint that
        /// saved a file of the project.
        fn log(&self, dir: &Path, cwd: &Path) -> (PathBuf, String) {
            let session = Session::create(dir, cwd).unwrap();
            let id = session.header().id.clone();
            let run = dir.join(CHECKPOINTS_DIR).join(&id).join("0");
            std::fs::create_dir_all(&run).unwrap();
            let manifest = serde_json::json!({
                "leaf_before": null,
                "files": [{
                    "path": self.from_root.join("src/a.rs"),
                    "existed": true,
                    "copy": run.join("a.rs"),
                }],
            });
            std::fs::write(run.join("manifest.json"), manifest.to_string()).unwrap();
            std::fs::write(run.join("a.rs"), "old").unwrap();
            (session.path().to_path_buf(), id)
        }
    }

    fn header(path: &Path) -> Value {
        let text = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    fn manifest_files(path: &Path) -> Vec<(String, String)> {
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["path"].as_str().unwrap().to_string(),
                    f["copy"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// A move keeps the logs' names and ids, points their headers and
    /// checkpoints at the new root, and reports where each log went; the
    /// logs of directories inside the project go along.
    #[test]
    fn a_move_carries_the_logs_and_rewrites_their_paths() {
        let fx = Fixture::new();
        let (root_log, id) = fx.log(&fx.from_dir, &fx.from_root);
        let (sub_log, _) = fx.log(&fx.from_dir.join("sub"), &fx.from_root.join("sub"));

        let moved = relocate_sessions(fx.relocation(false)).unwrap();

        let new_root_log = fx.to_dir.join(root_log.file_name().unwrap());
        let new_sub_log = fx.to_dir.join("sub").join(sub_log.file_name().unwrap());
        assert_eq!(moved.get(&root_log), Some(&new_root_log));
        assert_eq!(moved.get(&sub_log), Some(&new_sub_log));
        assert!(!fx.from_dir.exists());
        assert_eq!(header(&new_root_log)["cwd"], "/b/new");
        assert_eq!(header(&new_root_log)["id"], id.as_str());
        assert_eq!(header(&new_sub_log)["cwd"], "/b/new/sub");
        let run = fx.to_dir.join(CHECKPOINTS_DIR).join(&id).join("0");
        assert_eq!(
            manifest_files(&run.join("manifest.json")),
            [(
                "/b/new/src/a.rs".to_string(),
                run.join("a.rs").display().to_string()
            )]
        );
        // The moved log opens as a session.
        let reopened = Session::open(&new_root_log).unwrap();
        assert_eq!(reopened.header().cwd, PathBuf::from("/b/new"));
    }

    /// A copy leaves the original logs alone and gives each copied log a
    /// new id — in its name, its header and its checkpoints' directory —
    /// and a delegated log the copied parent's.
    #[test]
    fn a_copy_gives_the_logs_new_ids() {
        let fx = Fixture::new();
        let (parent_log, parent_id) = fx.log(&fx.from_dir, &fx.from_root);
        let delegate = Session::create_delegate(
            &fx.from_dir,
            &fx.from_root,
            crate::session::SessionParent {
                session: Some(parent_id.clone()),
                agent: "default".into(),
                call: "c1".into(),
            },
        )
        .unwrap();
        let delegate_log = delegate.path().to_path_buf();
        drop(delegate);

        let copied = relocate_sessions(fx.relocation(true)).unwrap();

        assert!(parent_log.exists() && delegate_log.exists());
        assert_eq!(header(&parent_log)["cwd"], "/a/proj");
        let new_parent = &copied[&parent_log];
        let new_delegate = &copied[&delegate_log];
        assert_ne!(new_parent.file_name(), parent_log.file_name());
        let new_id = header(new_parent)["id"].as_str().unwrap().to_string();
        assert_ne!(new_id, parent_id);
        assert!(new_parent
            .to_string_lossy()
            .ends_with(&format!("_{new_id}.jsonl")));
        assert_eq!(header(new_delegate)["parent"]["session"], new_id.as_str());
        assert_eq!(header(new_delegate)["cwd"], "/b/new");
        assert!(!fx.to_dir.join(CHECKPOINTS_DIR).join(&parent_id).exists());
        let run = fx.to_dir.join(CHECKPOINTS_DIR).join(&new_id).join("0");
        assert_eq!(
            manifest_files(&run.join("manifest.json")),
            [(
                "/b/new/src/a.rs".to_string(),
                run.join("a.rs").display().to_string()
            )]
        );
        // The lock the delegate held is not copied.
        let locks = std::fs::read_dir(&fx.to_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "lock"))
            .count();
        assert_eq!(locks, 0);
    }
}
