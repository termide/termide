//! Git commit information and log functions.
//!
//! Provides types and functions for working with git commit history.

use std::path::Path;

use crate::command::git_command_stdout;

/// Commit information
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// Commit hash (short form)
    pub hash: String,
    /// Author name
    pub author: String,
    /// Commit date
    pub date: String,
    /// Commit message (first line)
    pub message: String,
    /// Graph line for display (if using --graph)
    pub graph: Option<String>,
    /// Refs pointing to this commit (HEAD, branches, tags)
    pub refs: Option<String>,
}

/// Options that keep a patch in git's own format whatever the user's config
/// says: no external diff tool (`diff.external`, e.g. difftastic), no colour,
/// and the `a/` `b/` prefixes `diff.noprefix` and `diff.mnemonicPrefix` would
/// change. The diff panel parses what comes out.
pub(crate) const PLAIN_PATCH: [&str; 4] = [
    "--no-ext-diff",
    "--no-color",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// Get diff for a specific commit (with full patch)
pub fn get_commit_diff(repo: &Path, hash: &str) -> Option<String> {
    let mut args = vec!["show", "--stat", "--patch"];
    args.extend(PLAIN_PATCH);
    args.push(hash);
    git_command_stdout(repo, &args)
}

/// Get file diff (for diff viewer)
pub fn get_file_diff(repo: &Path, file: &Path, staged: bool) -> Option<String> {
    let file_str = file.to_string_lossy();
    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    args.extend(PLAIN_PATCH);
    args.extend(["--", &file_str]);
    git_command_stdout(repo, &args)
}

/// Detailed commit information for modal display.
#[derive(Debug, Clone)]
pub struct CommitDetails {
    /// Full commit hash.
    pub hash: String,
    /// Author as "Name <email>".
    pub author: String,
    /// Absolute date (ISO format).
    pub date: String,
    /// Full commit message (subject + body).
    pub message: String,
    /// Number of files changed.
    pub files_changed: usize,
    /// Number of insertions.
    pub insertions: usize,
    /// Number of deletions.
    pub deletions: usize,
    /// Number of files added.
    pub files_added: usize,
    /// Number of files deleted.
    pub files_deleted: usize,
    /// Number of files modified.
    pub files_modified: usize,
}

/// Get detailed information about a specific commit.
pub fn get_commit_details(repo: &Path, short_hash: &str) -> Option<CommitDetails> {
    let output = git_command_stdout(
        repo,
        &[
            "show",
            "--format=%H%n%an <%ae>%n%ai%n%B",
            "--shortstat",
            short_hash,
        ],
    )?;

    // The output has: format lines, then diff content, then shortstat line at the end.
    // Format: hash\nauthor\ndate\nmessage_lines...\n\n shortstat_line
    let mut lines = output.lines();

    let hash = lines.next()?.to_string();
    let author = lines.next()?.to_string();
    let date = lines.next()?.to_string();

    // Collect message lines until we hit an empty line followed by diff/shortstat.
    // The %B format ends with an empty line, then git show appends diff output.
    // We need to collect the message and find the shortstat line at the very end.
    let remaining: Vec<&str> = lines.collect();

    // The shortstat line is the last non-empty line.
    // Message is everything between current position and the diff/shortstat section.
    // Find shortstat line (contains "file(s) changed" or is empty for no changes).
    let mut files_changed = 0;
    let mut insertions = 0;
    let mut deletions = 0;
    let mut shortstat_idx = None;

    for (i, line) in remaining.iter().enumerate().rev() {
        let trimmed = line.trim();
        if trimmed.contains("changed") && (trimmed.contains("file") || trimmed.contains("files")) {
            // Parse shortstat: " 3 files changed, 10 insertions(+), 5 deletions(-)"
            for part in trimmed.split(',') {
                let part = part.trim();
                if part.contains("file") {
                    files_changed = part
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                } else if part.contains("insertion") {
                    insertions = part
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                } else if part.contains("deletion") {
                    deletions = part
                        .split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                }
            }
            shortstat_idx = Some(i);
            break;
        }
    }

    // Message: lines from start of remaining until diff content begins.
    // The message from %B ends with a trailing newline, so we trim trailing empty lines.
    // After the message, there may be diff lines before shortstat.
    // We take lines up to the first diff header ("diff --git") or shortstat.
    let message_end = remaining
        .iter()
        .position(|line| line.starts_with("diff --git"))
        .or(shortstat_idx)
        .unwrap_or(remaining.len());

    let message = remaining[..message_end].join("\n").trim_end().to_string();

    // Get per-file status breakdown (added/deleted/modified)
    let (files_added, files_deleted, files_modified) = if let Some(output) = git_command_stdout(
        repo,
        &[
            "diff-tree",
            "--no-commit-id",
            "-r",
            "--name-status",
            short_hash,
        ],
    ) {
        let mut a = 0;
        let mut d = 0;
        let mut m = 0;
        for line in output.lines() {
            match line.chars().next() {
                Some('A') => a += 1,
                Some('D') => d += 1,
                _ => m += 1, // M, R, T, etc. -> modified
            }
        }
        (a, d, m)
    } else {
        (0, 0, files_changed)
    };

    Some(CommitDetails {
        hash,
        author,
        date,
        message,
        files_changed,
        insertions,
        deletions,
        files_added,
        files_deleted,
        files_modified,
    })
}

/// Diff statistics for a file.
#[derive(Debug, Clone, Default)]
pub struct DiffStats {
    /// Number of lines added.
    pub additions: usize,
    /// Number of lines deleted.
    pub deletions: usize,
}

/// Get diff stats for a file (additions/deletions count).
pub fn get_file_diff_stats(repo: &Path, file: &Path, staged: bool) -> DiffStats {
    let file_str = file.to_string_lossy();
    let args: Vec<&str> = if staged {
        vec!["diff", "--cached", "--numstat", "--", &file_str]
    } else {
        vec!["diff", "--numstat", "--", &file_str]
    };

    let output = git_command_stdout(repo, &args);

    // Parse: "10\t5\tfilename" -> additions=10, deletions=5
    if let Some(text) = output {
        if let Some(line) = text.lines().next() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                return DiffStats {
                    additions: parts[0].parse().unwrap_or(0),
                    deletions: parts[1].parse().unwrap_or(0),
                };
            }
        }
    }
    DiffStats::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// The diff panel parses git's own patch format; a user's external diff
    /// tool (difftastic is a common one), forced colour or dropped `a/` `b/`
    /// prefixes would hand it something else.
    #[test]
    fn diffs_for_the_panel_ignore_the_user_s_diff_settings() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f.txt"), "one\n").unwrap();
        git(repo, &["add", "f.txt"]);
        git(repo, &["commit", "-q", "-m", "one"]);
        std::fs::write(repo.join("f.txt"), "one\ntwo\n").unwrap();
        git(repo, &["add", "f.txt"]);
        std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
        for (key, value) in [
            ("diff.external", "echo EXTERNAL"),
            ("color.diff", "always"),
            ("color.ui", "always"),
            ("diff.noprefix", "true"),
            ("diff.mnemonicPrefix", "true"),
        ] {
            git(repo, &["config", key, value]);
        }

        let patches = [
            get_file_diff(repo, Path::new("f.txt"), false),
            get_file_diff(repo, Path::new("f.txt"), true),
            get_commit_diff(repo, "HEAD"),
        ];
        for patch in patches {
            let patch = patch.expect("a diff");
            assert!(patch.contains("diff --git a/f.txt b/f.txt"), "{patch}");
            assert!(patch.contains("+++ b/f.txt"), "{patch}");
            assert!(!patch.contains("EXTERNAL"), "{patch}");
            assert!(!patch.contains('\x1b'), "{patch}");
        }
    }
}
