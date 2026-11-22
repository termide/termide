//! Git commit information and log functions.
//!
//! Provides types and functions for working with git commit history.

use std::path::Path;

use crate::command::{git_command_stdout, git_command_stdout_bytes};

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
        &["show", "-s", "--format=%H%n%an <%ae>%n%ai%n%B", short_hash],
    )?;
    let mut lines = output.lines();
    let hash = lines.next()?.to_string();
    let author = lines.next()?.to_string();
    let date = lines.next()?.to_string();
    let message = lines.collect::<Vec<_>>().join("\n").trim_end().to_string();

    // Counted from `--numstat`: the `--shortstat` summary is worded in the
    // user's language.
    let (files_changed, insertions, deletions) =
        git_command_stdout_bytes(repo, &["show", "--format=", "--numstat", short_hash])
            .map(|out| sum_numstat(&String::from_utf8_lossy(&out)))
            .unwrap_or_default();

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

/// Files, insertions and deletions in `--numstat` output (`added\tdeleted\tpath`
/// a line; `-` for both counts of a binary file).
pub(crate) fn sum_numstat(output: &str) -> (usize, usize, usize) {
    let mut totals = (0, 0, 0);
    for line in output.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(_)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        totals.0 += 1;
        totals.1 += added.parse::<usize>().unwrap_or(0);
        totals.2 += deleted.parse::<usize>().unwrap_or(0);
    }
    totals
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

    #[test]
    fn numstat_sums_files_and_lines_whatever_the_language() {
        let out = "3\t1\tsrc/a.rs\n-\t-\tlogo.png\n0\t2\told.rs => new.rs\n\n";
        assert_eq!(sum_numstat(out), (3, 3, 3));
        assert_eq!(sum_numstat(""), (0, 0, 0));
    }

    #[test]
    fn commit_details_count_the_changes_and_keep_the_whole_message() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.join("b.txt"), "gone\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::remove_file(repo.join("b.txt")).unwrap();
        std::fs::write(repo.join("c.txt"), "new\n").unwrap();
        git(repo, &["add", "-A"]);
        // A body line that reads like a summary must stay in the message.
        git(
            repo,
            &[
                "commit",
                "-q",
                "-m",
                "Change",
                "-m",
                "2 files changed, 1 insertion(+)",
            ],
        );

        let details = get_commit_details(repo, "HEAD").unwrap();
        assert_eq!(details.message, "Change\n\n2 files changed, 1 insertion(+)");
        assert_eq!(details.files_changed, 3);
        assert_eq!(details.insertions, 3);
        assert_eq!(details.deletions, 2);
        assert_eq!(
            (
                details.files_added,
                details.files_deleted,
                details.files_modified
            ),
            (1, 1, 1)
        );
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
