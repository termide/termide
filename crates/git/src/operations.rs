//! Git operations for staging, committing, and syncing.
//!
//! Provides high-level git operations like stage, unstage, commit, push, pull.

use std::path::{Path, PathBuf};

use crate::command::{hardened_git, run_git_simple, run_git_with_stderr};

/// Stage a file (add to index)
#[must_use = "staging result must be checked"]
pub fn stage_file(repo: &Path, file: &Path) -> Result<(), String> {
    let file_str = file.to_string_lossy();
    run_git_simple(
        repo,
        &["add", &file_str],
        &format!("Failed to stage file: {}", file_str),
    )
}

/// Stage multiple files
#[must_use = "staging result must be checked"]
pub fn stage_files(repo: &Path, files: &[PathBuf]) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }

    let mut args = vec!["add", "--"];
    let file_strs: Vec<String> = files
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    args.extend(file_strs.iter().map(|s| s.as_str()));

    run_git_simple(repo, &args, "Failed to stage files")
}

/// Unstage a file (remove from index)
#[must_use = "unstaging result must be checked"]
pub fn unstage_file(repo: &Path, file: &Path) -> Result<(), String> {
    let file_str = file.to_string_lossy();
    run_git_simple(
        repo,
        &["reset", "HEAD", "--", &file_str],
        &format!("Failed to unstage file: {}", file_str),
    )
}

/// Unstage multiple files
#[must_use = "unstaging result must be checked"]
pub fn unstage_files(repo: &Path, files: &[PathBuf]) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }

    let mut args = vec!["reset", "HEAD", "--"];
    let file_strs: Vec<String> = files
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    args.extend(file_strs.iter().map(|s| s.as_str()));

    run_git_simple(repo, &args, "Failed to unstage files")
}

/// Stage all changes
#[must_use = "staging result must be checked"]
pub fn stage_all(repo: &Path) -> Result<(), String> {
    run_git_simple(repo, &["add", "-A"], "Failed to stage all files")
}

/// Unstage all changes
#[must_use = "unstaging result must be checked"]
pub fn unstage_all(repo: &Path) -> Result<(), String> {
    run_git_simple(repo, &["reset", "HEAD"], "Failed to unstage all files")
}

/// Create a commit and return its abbreviated hash.
///
/// Runs the repository's hooks, so it can take long: call it off the UI
/// thread. The error is git's own message, without a prefix.
#[must_use = "commit result must be checked"]
pub fn commit(repo: &Path, message: &str) -> Result<String, String> {
    let output = hardened_git(repo, &["commit", "-m", message])
        .output()
        .map_err(|e| format!("Failed to run git commit: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // "nothing to commit" and some hook messages go to stdout.
        let text = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(text.to_string());
    }
    // The summary line's last word is the subject's, not the hash, so ask
    // for the hash outright.
    let head = hardened_git(repo, &["rev-parse", "--short", "HEAD"])
        .output()
        .map_err(|e| format!("Failed to run git rev-parse: {}", e))?;
    Ok(String::from_utf8_lossy(&head.stdout).trim().to_string())
}

/// Revert changes in a file (restore from HEAD)
#[must_use = "revert result must be checked"]
pub fn revert_file(repo: &Path, file: &Path) -> Result<(), String> {
    let file_str = file.to_string_lossy();
    run_git_simple(
        repo,
        &["checkout", "--", &file_str],
        &format!("Failed to revert file: {}", file_str),
    )
}

/// Revert all local changes (restore tracked files + clean untracked)
#[must_use = "revert result must be checked"]
pub fn revert_all(repo: &Path) -> Result<(), String> {
    // Restore all tracked files to HEAD state
    run_git_simple(repo, &["checkout", "--", "."], "Failed to revert changes")?;
    // Remove untracked files and directories
    run_git_simple(repo, &["clean", "-fd"], "Failed to clean untracked files")?;
    Ok(())
}

/// Initialize a new git repository
#[must_use = "init result must be checked"]
pub fn init_repo(path: &Path) -> Result<(), String> {
    run_git_with_stderr(path, &["init"], "init")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.name", "t"]);
        git(dir.path(), &["config", "user.email", "t@t"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        dir
    }

    #[test]
    fn a_commit_reports_its_hash_not_the_subject_s_last_word() {
        let dir = repo();
        std::fs::write(dir.path().join("f"), "x").unwrap();
        stage_all(dir.path()).unwrap();
        // A subject ending in a multi-byte word once came back as the
        // "hash" and was cut mid-character.
        let hash = commit(dir.path(), "Исправить проверку").unwrap();
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        assert!(
            !hash.is_empty() && head.starts_with(&hash),
            "{hash} vs {head}"
        );
    }

    #[test]
    fn a_failed_commit_says_why() {
        let dir = repo();
        let err = commit(dir.path(), "empty").unwrap_err();
        // git says "nothing to commit" on stdout, in the user's language.
        assert!(!err.is_empty());
        assert!(!err.starts_with("Commit failed"), "{err}");
    }
}
