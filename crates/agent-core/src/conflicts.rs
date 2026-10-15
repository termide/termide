//! Agents in different panels working on the same files: an edit made on
//! what a file held before another panel changed it would undo that change
//! unseen. Rather than guess when to keep agents apart, the change is caught
//! exactly: every panel's reads and writes of a file are noted in order,
//! process-wide, and an `edit` or `write` of a file another panel changed
//! after this one last read it — or that this one never read here — is
//! refused, the model told to read it again first. A call from outside a
//! panel (headless) is neither checked nor noted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::agent::{Hooks, ToolDecision};
use crate::message::{ToolCall, ToolResultMessage};
use crate::tool::ToolContext;

/// Who did what to which file, in the order it happened.
#[derive(Default)]
struct Ledger {
    /// Bumped on every write: the order of events.
    clock: u64,
    /// A file's last write: the panel that made it, and when.
    writes: HashMap<PathBuf, (u64, u64)>,
    /// When a panel last read or wrote a file.
    seen: HashMap<(u64, PathBuf), u64>,
}

fn ledger() -> &'static Mutex<Ledger> {
    static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
    LEDGER.get_or_init(Mutex::default)
}

/// One file under however it is spelled: canonical when it exists, its
/// directory's canonical path joined with its name for a file still to be
/// made, else as given.
fn key(path: &Path) -> PathBuf {
    if let Ok(path) = dunce::canonicalize(path) {
        return path;
    }
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) => dunce::canonicalize(dir)
            .map(|dir| dir.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// What a call does to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

/// The guard for one run: it checks a change before it runs and notes a
/// read or a change once it succeeded.
#[derive(Default)]
pub struct ConflictGuard {
    /// The calls under way: their panel, file and access.
    pending: HashMap<String, (u64, PathBuf, Access)>,
}

impl ConflictGuard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Hooks for ConflictGuard {
    fn before_tool_call(&mut self, call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        let access = match call.name.as_str() {
            "read" => Access::Read,
            "edit" | "write" => Access::Write,
            _ => return ToolDecision::Allow,
        };
        let (Some(panel), Some(raw)) = (ctx.panel, call.arguments["path"].as_str()) else {
            return ToolDecision::Allow;
        };
        if raw.trim().is_empty() {
            return ToolDecision::Allow;
        }
        let path = Path::new(raw);
        let path = key(&if path.is_absolute() {
            path.to_path_buf()
        } else {
            ctx.cwd.join(path)
        });
        if access == Access::Write {
            let ledger = ledger().lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(&(writer, at)) = ledger.writes.get(&path) {
                let seen = ledger.seen.get(&(panel, path.clone())).copied();
                if writer != panel && seen.is_none_or(|seen| seen < at) {
                    return ToolDecision::Block {
                        reason: format!(
                            "The agent in panel {writer} changed {} after you last read it here. \
                             Read the file again, then make your change on what it holds now.",
                            path.display()
                        ),
                    };
                }
            }
        }
        self.pending.insert(call.id.clone(), (panel, path, access));
        ToolDecision::Allow
    }

    fn after_tool_call(&mut self, call: &ToolCall, result: ToolResultMessage) -> ToolResultMessage {
        let Some((panel, path, access)) = self.pending.remove(&call.id) else {
            return result;
        };
        if result.is_error {
            return result;
        }
        let mut ledger = ledger().lock().unwrap_or_else(PoisonError::into_inner);
        if access == Access::Write {
            ledger.clock += 1;
            let at = ledger.clock;
            ledger.writes.insert(path.clone(), (panel, at));
        }
        let now = ledger.clock;
        ledger.seen.insert((panel, path), now);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str, path: &Path) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: json!({ "path": path.to_string_lossy() }),
            extra_content: None,
        }
    }

    /// Run `name` on `path` from `panel` through `guard`; whether it ran.
    fn run(
        guard: &mut ConflictGuard,
        panel: Option<u64>,
        id: &str,
        name: &str,
        path: &Path,
    ) -> bool {
        let call = call(id, name, path);
        let ctx = ToolContext {
            panel,
            ..ToolContext::new(path.parent().unwrap())
        };
        if let ToolDecision::Block { reason } = guard.before_tool_call(&call, &ctx) {
            assert!(reason.contains("Read the file again"), "{reason}");
            return false;
        }
        guard.after_tool_call(&call, ToolResultMessage::text(&call, "ok"));
        true
    }

    #[test]
    fn a_change_on_a_file_another_panel_changed_since_waits_for_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        std::fs::write(&file, "fn a() {}").unwrap();
        let (mut one, mut two) = (ConflictGuard::new(), ConflictGuard::new());
        // Panels numbered far from the other tests', the ledger being shared.
        let (a, b) = (Some(9001), Some(9002));
        assert!(run(&mut one, a, "1", "read", &file));
        assert!(run(&mut two, b, "2", "read", &file));
        assert!(run(&mut one, a, "3", "edit", &file));
        // The other panel read it before that change: it must read again.
        assert!(!run(&mut two, b, "4", "edit", &file));
        assert!(!run(&mut two, b, "5", "write", &file));
        assert!(run(&mut two, b, "6", "read", &file));
        assert!(run(&mut two, b, "7", "edit", &file));
        // Its own change does not stand in its way; the first panel's turn
        // to read.
        assert!(run(&mut two, b, "8", "edit", &file));
        assert!(!run(&mut one, a, "9", "edit", &file));
        // A panel that never read a file another one changed reads first.
        let fresh = dir.path().join("new.rs");
        assert!(run(&mut one, a, "10", "write", &fresh));
        assert!(!run(&mut two, b, "11", "write", &fresh));
        // Outside a panel nothing is checked.
        assert!(run(&mut ConflictGuard::new(), None, "12", "edit", &file));
    }

    #[test]
    fn a_failed_call_notes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let (mut one, mut two) = (ConflictGuard::new(), ConflictGuard::new());
        let (a, b) = (Some(9101), Some(9102));
        assert!(run(&mut two, b, "1", "read", &file));
        let edit = call("2", "edit", &file);
        let ctx = ToolContext {
            panel: a,
            ..ToolContext::new(dir.path())
        };
        assert_eq!(one.before_tool_call(&edit, &ctx), ToolDecision::Allow);
        one.after_tool_call(&edit, ToolResultMessage::error(&edit, "no match"));
        assert!(run(&mut two, b, "3", "edit", &file));
    }
}
