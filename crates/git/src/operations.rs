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

/// The sources of the staged renames whose destination is one of `paths`
/// (relative to `repo` or absolute) or lies under one, as top-level
/// pathspecs. A rename is listed under its destination only, and resetting
/// that alone would leave its source staged as deleted.
fn staged_rename_sources(repo: &Path, paths: &[PathBuf]) -> Vec<String> {
    let renames = crate::files::staged_renames(repo);
    if renames.is_empty() {
        return Vec::new();
    }
    let Some(top) = crate::command::git_command_stdout(repo, &["rev-parse", "--show-toplevel"])
    else {
        return Vec::new();
    };
    let top = dunce::canonicalize(top.trim()).unwrap_or_else(|_| PathBuf::from(top.trim()));
    let repo = dunce::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let relative: Vec<PathBuf> = paths
        .iter()
        .filter_map(|path| {
            let absolute = repo.join(path);
            let absolute = dunce::canonicalize(&absolute).unwrap_or(absolute);
            absolute.strip_prefix(&top).ok().map(Path::to_path_buf)
        })
        .collect();
    renames
        .into_iter()
        .filter(|(_, destination)| relative.iter().any(|path| destination.starts_with(path)))
        .map(|(source, _)| format!(":(top,literal){}", source.to_string_lossy()))
        .collect()
}

/// Unstage a file (remove from index); a staged rename's source with it.
#[must_use = "unstaging result must be checked"]
pub fn unstage_file(repo: &Path, file: &Path) -> Result<(), String> {
    unstage_files(repo, &[file.to_path_buf()])
        .map_err(|_| format!("Failed to unstage file: {}", file.to_string_lossy()))
}

/// Unstage multiple files; the sources of staged renames among them too.
#[must_use = "unstaging result must be checked"]
pub fn unstage_files(repo: &Path, files: &[PathBuf]) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }

    let mut args = vec!["reset", "HEAD", "--"];
    let mut file_strs: Vec<String> = files
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    file_strs.extend(staged_rename_sources(repo, files));
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

/// Revert a staged file: unstage it, then restore it from HEAD. A staged
/// rename is undone whole: its source comes back and its destination, which
/// HEAD does not hold, is removed.
#[must_use = "revert result must be checked"]
pub fn revert_staged_file(repo: &Path, file: &Path) -> Result<(), String> {
    let sources = staged_rename_sources(repo, &[file.to_path_buf()]);
    unstage_file(repo, file)?;
    if sources.is_empty() {
        return revert_file(repo, file);
    }
    let mut args = vec!["checkout", "--"];
    args.extend(sources.iter().map(String::as_str));
    run_git_simple(
        repo,
        &args,
        &format!("Failed to revert file: {}", file.to_string_lossy()),
    )?;
    match std::fs::remove_file(repo.join(file)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!(
            "Failed to revert file: {}: {e}",
            file.to_string_lossy()
        )),
        _ => Ok(()),
    }
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

    /// A repository with `f` committed and `git mv f g` staged.
    fn renamed() -> tempfile::TempDir {
        let dir = repo();
        std::fs::write(dir.path().join("f"), "content\n").unwrap();
        git(dir.path(), &["add", "f"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        git(dir.path(), &["mv", "f", "g"]);
        dir
    }

    fn porcelain(dir: &Path) -> String {
        git(dir, &["status", "--porcelain=v1", "--untracked-files=all"])
    }

    #[test]
    fn unstaging_a_rename_unstages_its_source_too() {
        let dir = renamed();
        unstage_file(dir.path(), Path::new("g")).unwrap();
        // Not `D  f` left staged beside `?? g`.
        assert_eq!(porcelain(dir.path()), " D f\n?? g\n");

        let dir = renamed();
        unstage_files(dir.path(), &[PathBuf::from("g")]).unwrap();
        assert_eq!(porcelain(dir.path()), " D f\n?? g\n");

        // By its absolute path, as the file manager names it.
        let dir = renamed();
        unstage_file(dir.path(), &dir.path().join("g")).unwrap();
        assert_eq!(porcelain(dir.path()), " D f\n?? g\n");
    }

    #[test]
    fn reverting_a_staged_rename_brings_the_source_back() {
        let dir = renamed();
        revert_staged_file(dir.path(), Path::new("g")).unwrap();
        assert_eq!(porcelain(dir.path()), "");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f")).unwrap(),
            "content\n"
        );
        assert!(!dir.path().join("g").exists());
    }

    #[test]
    fn reverting_a_staged_modification_restores_head() {
        let dir = repo();
        std::fs::write(dir.path().join("f"), "one\n").unwrap();
        git(dir.path(), &["add", "f"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("f"), "two\n").unwrap();
        git(dir.path(), &["add", "f"]);
        revert_staged_file(dir.path(), Path::new("f")).unwrap();
        assert_eq!(porcelain(dir.path()), "");
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
