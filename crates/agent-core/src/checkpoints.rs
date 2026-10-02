//! Checkpoints: what every file the agent touches looked like before one
//! request changed it, kept so the request can be undone — the files put
//! back and the conversation rewound to before it, the way Claude Code's
//! checkpoints and OpenCode's `/undo` work.
//!
//! Copies live under the session's directory, one folder per request, with
//! a manifest naming the original paths; a file that did not exist before is
//! recorded as such and removed again on undo. Nothing depends on git, so
//! it works in any directory and for files git ignores.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{Hooks, ToolDecision};
use crate::message::ToolCall;
use crate::tool::ToolContext;

/// One file as it was before the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFile {
    /// The file's own path.
    pub path: PathBuf,
    /// `false` when the request created it; undo removes it then.
    pub existed: bool,
    /// The copy, when `existed`.
    pub copy: Option<PathBuf>,
}

/// One request's checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Run {
    /// The session's leaf before the request's first message.
    leaf_before: Option<String>,
    files: Vec<SavedFile>,
}

/// An undoable request as [`CheckpointStore::checkpoints`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The files it changed.
    pub files: Vec<PathBuf>,
    /// The session's leaf before the request's first message.
    pub leaf_before: Option<String>,
}

/// What an undo did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undone {
    pub files: Vec<PathBuf>,
    pub leaf_before: Option<String>,
}

pub struct CheckpointStore {
    dir: PathBuf,
    /// Requests that changed files, oldest first, with their folder index.
    runs: Vec<(usize, Run)>,
    /// The request in progress, with its folder index.
    current: Option<(usize, Run)>,
    next_index: usize,
}

impl CheckpointStore {
    /// Open the store at `dir`, reading the manifests already there.
    #[must_use]
    pub fn open(dir: PathBuf) -> Self {
        let mut runs: Vec<(usize, Run)> = Vec::new();
        if let Ok(read_dir) = std::fs::read_dir(&dir) {
            for entry in read_dir.flatten() {
                let Some(index) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<usize>().ok())
                else {
                    continue;
                };
                let manifest = entry.path().join("manifest.json");
                let Ok(text) = std::fs::read_to_string(&manifest) else {
                    continue;
                };
                match serde_json::from_str::<Run>(&text) {
                    Ok(run) => runs.push((index, run)),
                    Err(error) => log::warn!("ignoring {}: {error}", manifest.display()),
                }
            }
        }
        runs.sort_by_key(|(index, _)| *index);
        let next_index = runs.last().map_or(0, |(index, _)| index + 1);
        Self {
            dir,
            runs,
            current: None,
            next_index,
        }
    }

    /// The store of `session_id` under `session_dir`.
    #[must_use]
    pub fn for_session(session_dir: &Path, session_id: &str) -> Self {
        Self::open(session_dir.join("checkpoints").join(session_id))
    }

    /// A request starts; `leaf_before` is where the conversation stood.
    pub fn begin_run(&mut self, leaf_before: Option<String>) {
        self.end_run();
        self.current = Some((
            self.next_index,
            Run {
                leaf_before,
                files: Vec::new(),
            },
        ));
        self.next_index += 1;
    }

    /// The request ended; it is kept only if it changed a file.
    pub fn end_run(&mut self) {
        if let Some((index, run)) = self.current.take() {
            if run.files.is_empty() {
                self.next_index = index;
            } else {
                self.runs.push((index, run));
            }
        }
    }

    /// Keep `path` as it is now, before the request changes it. Once per
    /// request per file; a no-op outside a request.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        let Some((index, run)) = &mut self.current else {
            return Ok(());
        };
        if run.files.iter().any(|f| f.path == path) {
            return Ok(());
        }
        let folder = self.dir.join(index.to_string());
        std::fs::create_dir_all(&folder)?;
        let saved = if path.is_file() {
            let copy = folder.join(format!("{}.orig", run.files.len()));
            std::fs::copy(path, &copy)?;
            SavedFile {
                path: path.to_path_buf(),
                existed: true,
                copy: Some(copy),
            }
        } else {
            SavedFile {
                path: path.to_path_buf(),
                existed: false,
                copy: None,
            }
        };
        run.files.push(saved);
        std::fs::write(
            folder.join("manifest.json"),
            serde_json::to_string_pretty(run).unwrap_or_default(),
        )
    }

    /// Requests that can be undone.
    #[must_use]
    pub fn undoable(&self) -> usize {
        self.runs.len()
            + usize::from(
                self.current
                    .as_ref()
                    .is_some_and(|(_, r)| !r.files.is_empty()),
            )
    }

    /// The files the last undoable request changed.
    #[must_use]
    pub fn last_files(&self) -> Vec<PathBuf> {
        self.current
            .as_ref()
            .filter(|(_, r)| !r.files.is_empty())
            .or(self.runs.last())
            .map(|(_, r)| r.files.iter().map(|f| f.path.clone()).collect())
            .unwrap_or_default()
    }

    /// The undoable checkpoints, newest first — for offering a rollback
    /// target. The order matches repeated [`CheckpointStore::undo_last`]
    /// calls: the in-progress run (when it has changed files) first, then the
    /// finished runs from newest to oldest.
    #[must_use]
    pub fn checkpoints(&self) -> Vec<Checkpoint> {
        let current = self.current.as_ref().filter(|(_, r)| !r.files.is_empty());
        current
            .into_iter()
            .chain(self.runs.iter().rev())
            .map(|(_, run)| Checkpoint {
                files: run.files.iter().map(|f| f.path.clone()).collect(),
                leaf_before: run.leaf_before.clone(),
            })
            .collect()
    }

    /// Drop the last request's checkpoint and leave its files as they are,
    /// for a conversation rewound past it: what it kept no longer belongs to
    /// the branch. `false` when there was nothing to drop.
    pub fn forget_last(&mut self) -> bool {
        let (index, _) = match self.current.take() {
            Some((index, run)) if !run.files.is_empty() => (index, run),
            other => {
                self.current = other;
                match self.runs.pop() {
                    Some(last) => last,
                    None => return false,
                }
            }
        };
        let _ = std::fs::remove_dir_all(self.dir.join(index.to_string()));
        true
    }

    /// Put the last request's files back and forget its checkpoint.
    pub fn undo_last(&mut self) -> Result<Undone, String> {
        let (index, run) = match self.current.take() {
            Some((index, run)) if !run.files.is_empty() => (index, run),
            other => {
                self.current = other;
                self.runs.pop().ok_or("nothing to undo")?
            }
        };
        let mut files = Vec::new();
        for saved in &run.files {
            let outcome = match (&saved.copy, saved.existed) {
                (Some(copy), true) => std::fs::copy(copy, &saved.path).map(|_| ()),
                _ => match std::fs::remove_file(&saved.path) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                },
            };
            match outcome {
                Ok(()) => files.push(saved.path.clone()),
                Err(error) => log::warn!("cannot restore {}: {error}", saved.path.display()),
            }
        }
        let _ = std::fs::remove_dir_all(self.dir.join(index.to_string()));
        Ok(Undone {
            files,
            leaf_before: run.leaf_before,
        })
    }
}

/// Records the files `edit` and `write` are about to touch. Placed first in
/// the hook chain, so a call the rules then deny is recorded too — harmless,
/// an unchanged file restores to itself — and none that runs is missed.
pub struct CheckpointHooks {
    store: Arc<Mutex<CheckpointStore>>,
}

impl CheckpointHooks {
    #[must_use]
    pub fn new(store: Arc<Mutex<CheckpointStore>>) -> Self {
        Self { store }
    }
}

impl Hooks for CheckpointHooks {
    fn before_tool_call(&mut self, call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        if matches!(call.name.as_str(), "edit" | "write") {
            if let Some(path) = call.arguments.get("path").and_then(Value::as_str) {
                let path = Path::new(path);
                let absolute = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    ctx.cwd.join(path)
                };
                if let Err(error) = self
                    .store
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .save(&absolute)
                {
                    log::warn!("cannot checkpoint {}: {error}", absolute.display());
                }
            }
        }
        ToolDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn files_are_saved_once_per_request_and_come_back_on_undo() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("a.txt");
        let created = tmp.path().join("new.txt");
        std::fs::write(&existing, "before").unwrap();
        let store = Arc::new(Mutex::new(CheckpointStore::for_session(tmp.path(), "s1")));
        let mut hooks = CheckpointHooks::new(Arc::clone(&store));
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let call = |name: &str, path: &str| ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: json!({ "path": path }),
            extra_content: None,
        };

        // Outside a request nothing is recorded.
        hooks.before_tool_call(&call("edit", "a.txt"), &ctx);
        assert_eq!(store.lock().unwrap().undoable(), 0);

        store.lock().unwrap().begin_run(Some("leaf-0".into()));
        hooks.before_tool_call(&call("edit", "a.txt"), &ctx);
        std::fs::write(&existing, "after").unwrap();
        hooks.before_tool_call(&call("edit", "a.txt"), &ctx); // second touch: not saved again
        hooks.before_tool_call(&call("write", "new.txt"), &ctx);
        std::fs::write(&created, "made").unwrap();
        hooks.before_tool_call(&call("read", "a.txt"), &ctx); // reads are not recorded
        store.lock().unwrap().end_run();
        assert_eq!(store.lock().unwrap().undoable(), 1);
        assert_eq!(
            store.lock().unwrap().last_files(),
            vec![existing.clone(), created.clone()]
        );

        // A request that changed nothing leaves no checkpoint.
        store.lock().unwrap().begin_run(Some("leaf-1".into()));
        store.lock().unwrap().end_run();
        assert_eq!(store.lock().unwrap().undoable(), 1);

        // The manifest survives a reopen.
        let mut reopened = CheckpointStore::for_session(tmp.path(), "s1");
        assert_eq!(reopened.undoable(), 1);
        let undone = reopened.undo_last().unwrap();
        assert_eq!(undone.leaf_before.as_deref(), Some("leaf-0"));
        assert_eq!(undone.files.len(), 2);
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "before");
        assert!(!created.exists());
        assert_eq!(reopened.undo_last().unwrap_err(), "nothing to undo");
        assert!(!tmp.path().join("checkpoints/s1/0").exists());
    }

    #[test]
    fn checkpoints_lists_undoable_runs_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let mut store = CheckpointStore::for_session(tmp.path(), "s1");

        store.begin_run(Some("leaf-0".into()));
        store.save(&a).unwrap();
        store.end_run();
        store.begin_run(Some("leaf-1".into()));
        store.save(&b).unwrap();
        store.end_run();

        let files = |store: &CheckpointStore| -> Vec<Vec<PathBuf>> {
            store.checkpoints().into_iter().map(|c| c.files).collect()
        };
        // Newest run (b) first, then the older (a).
        assert_eq!(files(&store), vec![vec![b.clone()], vec![a.clone()]]);
        assert_eq!(
            store.checkpoints()[0].leaf_before.as_deref(),
            Some("leaf-1")
        );
        assert_eq!(store.checkpoints().len(), store.undoable());

        // The in-progress run appears first once it has changed a file.
        store.begin_run(Some("leaf-2".into()));
        assert_eq!(store.checkpoints().len(), 2);
        store.save(&a).unwrap();
        assert_eq!(
            files(&store),
            vec![vec![a.clone()], vec![b.clone()], vec![a.clone()]]
        );
    }

    #[test]
    fn a_forgotten_checkpoint_leaves_its_files_as_they_are() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        std::fs::write(&a, "before").unwrap();
        let mut store = CheckpointStore::for_session(tmp.path(), "s1");
        store.begin_run(None);
        store.save(&a).unwrap();
        std::fs::write(&a, "after").unwrap();
        store.end_run();

        assert!(store.forget_last());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "after");
        assert_eq!(store.undoable(), 0);
        assert!(!tmp.path().join("checkpoints/s1/0").exists());
        assert!(!store.forget_last());
    }
}
