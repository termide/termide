//! Git stash operations.

use std::path::Path;

use super::command::{git_command_stdout, run_git_simple, run_git_with_stderr};

/// Format a stash ref string, e.g. `stash@{0}`.
fn stash_ref(index: usize) -> String {
    format!("stash@{{{}}}", index)
}

/// A single stash entry from `git stash list`.
#[derive(Debug, Clone)]
pub struct StashEntry {
    /// Stash index (0 = most recent)
    pub index: usize,
    /// Branch where stash was created
    pub branch: String,
    /// Human-readable message (the commit summary after the branch)
    pub message: String,
    /// Full ref string, e.g. `stash@{0}` — use this for git commands
    pub ref_str: String,
}

/// List all stash entries for the given repository.
///
/// Returns entries ordered by index (0 = most recent).
pub fn stash_list(repo: &Path) -> Vec<StashEntry> {
    let output = match git_command_stdout(repo, &["stash", "list"]) {
        Some(s) if !s.trim().is_empty() => s,
        _ => return Vec::new(),
    };

    output.lines().filter_map(parse_stash_line).collect()
}

/// Parse a single line from `git stash list`.
///
/// Format: `stash@{N}: WIP on branch: hash message`
///         `stash@{N}: On branch: custom message`
fn parse_stash_line(line: &str) -> Option<StashEntry> {
    // Extract ref_str (the part before ": ")
    let colon_pos = line.find(": ")?;
    let ref_str = line[..colon_pos].to_string();

    // Extract index from "stash@{N}"
    let index: usize = ref_str
        .strip_prefix("stash@{")?
        .strip_suffix('}')?
        .parse()
        .ok()?;

    // Rest after ": "
    let rest = &line[colon_pos + 2..];

    // Try to extract branch from "WIP on branch: ..." or "On branch: ..."
    let (branch, message) = if let Some(after_wip) = rest.strip_prefix("WIP on ") {
        if let Some(colon2) = after_wip.find(": ") {
            let branch = after_wip[..colon2].to_string();
            let msg = after_wip[colon2 + 2..].to_string();
            (branch, msg)
        } else {
            (after_wip.to_string(), String::new())
        }
    } else if let Some(after_on) = rest.strip_prefix("On ") {
        if let Some(colon2) = after_on.find(": ") {
            let branch = after_on[..colon2].to_string();
            let msg = after_on[colon2 + 2..].to_string();
            (branch, msg)
        } else {
            (after_on.to_string(), String::new())
        }
    } else {
        (String::new(), rest.to_string())
    };

    Some(StashEntry {
        index,
        branch,
        message,
        ref_str,
    })
}

/// Create a new stash with an optional message.
/// If `include_untracked` is true, also stash untracked files (-u flag).
pub fn stash_push(repo: &Path, message: &str, include_untracked: bool) -> Result<(), String> {
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("-u");
    }
    if !message.is_empty() {
        args.push("-m");
        args.push(message);
    }
    run_git_with_stderr(repo, &args, "stash push")
}

/// Pop (apply + drop) the stash at the given index.
pub fn stash_pop(repo: &Path, index: usize) -> Result<(), String> {
    let ref_str = stash_ref(index);
    run_git_simple(repo, &["stash", "pop", &ref_str], "Failed to pop stash")
}

/// Apply the stash at the given index (keep it in the stash list).
pub fn stash_apply(repo: &Path, index: usize) -> Result<(), String> {
    let ref_str = stash_ref(index);
    run_git_simple(repo, &["stash", "apply", &ref_str], "Failed to apply stash")
}

/// Drop (delete) the stash at the given index.
pub fn stash_drop(repo: &Path, index: usize) -> Result<(), String> {
    let ref_str = stash_ref(index);
    run_git_simple(repo, &["stash", "drop", &ref_str], "Failed to drop stash")
}

/// Rename a stash entry (change its message).
///
/// Git has no native "rename stash" command. This works by:
/// 1. Getting the commit hash of the stash
/// 2. Dropping the old stash entry
/// 3. Re-storing it with the new message via `git stash store`
pub fn stash_rename(repo: &Path, index: usize, new_message: &str) -> Result<(), String> {
    let ref_str = stash_ref(index);
    // Get the commit hash before dropping
    let hash = git_command_stdout(repo, &["rev-parse", &ref_str])
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("Failed to resolve {}", ref_str))?;
    // Drop the old entry
    run_git_simple(repo, &["stash", "drop", &ref_str], "Failed to drop stash")?;
    // Re-store with new message
    run_git_simple(
        repo,
        &["stash", "store", "-m", new_message, &hash],
        "Failed to store renamed stash",
    )
}

/// Get the full patch diff for a stash entry.
///
/// Runs `git stash show -p stash@{N}` which produces standard unified diff
/// output (with `diff --git` headers), suitable for the diff panel parser.
pub fn stash_diff(repo: &Path, stash_ref: &str) -> Option<String> {
    let mut args = vec!["stash", "show", "-p"];
    args.extend(crate::commits::PLAIN_PATCH);
    args.push(stash_ref);
    git_command_stdout(repo, &args)
}

/// Detailed information about a stash entry (for info modal).
#[derive(Debug, Clone)]
pub struct StashInfo {
    /// Human-readable message
    pub message: String,
    /// Creation date formatted as "YYYY-MM-DD HH:MM"
    pub date: String,
    /// Number of files changed
    pub files_changed: usize,
    /// Lines added
    pub insertions: usize,
    /// Lines removed
    pub deletions: usize,
    /// List of changed file paths
    pub file_names: Vec<String>,
}

/// Get detailed info about a stash entry for the info modal.
pub fn stash_info(repo: &Path, index: usize) -> Option<StashInfo> {
    let ref_str = stash_ref(index);

    // Get date
    let date_raw = git_command_stdout(repo, &["log", "-1", "--format=%ci", &ref_str])?;
    let date = date_raw.trim().chars().take(16).collect::<String>(); // "YYYY-MM-DD HH:MM"

    // Get message from stash list
    let list_output = git_command_stdout(repo, &["stash", "list"])?;
    let entry = list_output
        .lines()
        .find_map(parse_stash_line)
        .filter(|e| e.index == index);
    let message = entry.map(|e| e.message).unwrap_or_default();

    // Get file names
    let names_output =
        git_command_stdout(repo, &["stash", "show", "--name-only", &ref_str]).unwrap_or_default();
    let file_names: Vec<String> = names_output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    // Get diffstat (last line: "N files changed, M insertions(+), K deletions(-)")
    let stat_output =
        git_command_stdout(repo, &["stash", "show", "--stat", &ref_str]).unwrap_or_default();
    let (files_changed, insertions, deletions) = stat_output
        .lines()
        .last()
        .map(parse_diffstat_line)
        .unwrap_or((file_names.len(), 0, 0));

    Some(StashInfo {
        message,
        date,
        files_changed,
        insertions,
        deletions,
        file_names,
    })
}

/// Parse diffstat summary line: "5 files changed, 32 insertions(+), 14 deletions(-)"
fn parse_diffstat_line(line: &str) -> (usize, usize, usize) {
    let mut files = 0;
    let mut ins = 0;
    let mut del = 0;
    for part in line.split(',') {
        let part = part.trim();
        if part.contains("file") {
            files = part
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        } else if part.contains("insertion") {
            ins = part
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        } else if part.contains("deletion") {
            del = part
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        }
    }
    (files, ins, del)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_wip_on() {
        let line = "stash@{0}: WIP on main: abc1234 fix something";
        let entry = parse_stash_line(line).unwrap();
        assert_eq!(entry.index, 0);
        assert_eq!(entry.ref_str, "stash@{0}");
        assert_eq!(entry.branch, "main");
        assert_eq!(entry.message, "abc1234 fix something");
    }

    #[test]
    fn test_parse_on() {
        let line = "stash@{1}: On feature/foo: my custom message";
        let entry = parse_stash_line(line).unwrap();
        assert_eq!(entry.index, 1);
        assert_eq!(entry.branch, "feature/foo");
        assert_eq!(entry.message, "my custom message");
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse_stash_line("not a stash line").is_none());
    }
}
