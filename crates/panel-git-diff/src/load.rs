//! Diff loading and unified-diff parsing for the Git Diff Panel.

use std::path::Path;
use std::sync::{mpsc, LazyLock};
use std::thread;

use regex::Regex;

use termide_git::{self as git};

use crate::{DiffHunk, DiffLine, FileDiff, FileStatus, GitDiffPanel, LineKind};

static HUNK_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@").unwrap());

/// Snapshot produced by the background refresh worker.
///
/// Every git command a render needs runs on the worker thread; the UI thread
/// only swaps the result into place. Loading in the foreground cost `git
/// status` twice plus one `git diff` per changed file on the UI thread, and it
/// was paid not only when the panel opened: `OnFsUpdate` and `OnGitUpdate`
/// both refresh, so every watcher event in the repository went through it.
pub(crate) struct GitDiffRefreshResult {
    pub(crate) branch: Option<String>,
    pub(crate) diffs: Vec<FileDiff>,
    /// In a review, the files both the user and the agent edited.
    pub(crate) both: Vec<String>,
}

impl GitDiffPanel {
    /// Trigger a refresh of the diff.
    ///
    /// Returns immediately; the git commands run on a worker thread and the
    /// result is folded in by `tick()` via [`Self::poll_refresh`]. Until then
    /// the panel keeps showing the diff it already has, marked `is_loading`.
    pub fn refresh(&mut self) {
        // Coalesce: a worker is already running, so mark that one more pass is
        // needed when it finishes rather than spawning a parallel worker (and a
        // fresh batch of git subprocesses) for every queued watcher event.
        if self.refresh_rx.is_some() {
            self.refresh_pending = true;
            return;
        }
        self.is_loading = true;

        let repo = self.repo_path.clone();
        let commit_hash = self.commit_hash.clone();
        let is_stash = self.is_stash;
        let session = self.review.as_ref().map(|review| review.session.clone());

        let (tx, rx) = mpsc::channel();
        self.refresh_rx = Some(rx);
        thread::spawn(move || {
            let branch = git::get_current_branch(&repo);
            let (diffs, both) = match &session {
                Some(session) => load_review(&repo, session),
                None => (
                    load_diffs(&repo, commit_hash.as_deref(), is_stash),
                    Vec::new(),
                ),
            };
            let _ = tx.send(GitDiffRefreshResult {
                branch,
                diffs,
                both,
            });
        });
    }

    /// Apply an async refresh result if one is ready. Returns `true` when the
    /// panel state changed so the caller can emit `NeedsRedraw`.
    pub(crate) fn poll_refresh(&mut self) -> bool {
        let Some(rx) = self.refresh_rx.as_ref() else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return false,
            // The worker went away without sending. Free the receiver so the
            // next request can start instead of wedging on `is_loading`.
            Err(mpsc::TryRecvError::Disconnected) => {
                self.refresh_rx = None;
                self.is_loading = false;
                self.run_pending_refresh();
                return true;
            }
        };
        self.refresh_rx = None;

        self.branch = result.branch;
        self.diffs = result.diffs;
        if let Some(review) = self.review.as_mut() {
            review.both = result.both;
        }

        // Apply file filter if set
        if let Some(ref filter) = self.file_filter {
            self.diffs.retain(|d| &d.path == filter);
        }

        self.calculate_total_lines();
        self.settle_cursor();

        self.is_loading = false;
        self.run_pending_refresh();
        true
    }

    /// If a refresh was requested while a worker was in flight, run the single
    /// coalesced follow-up pass now that the receiver is free.
    fn run_pending_refresh(&mut self) {
        if self.refresh_pending {
            self.refresh_pending = false;
            self.refresh();
        }
    }

    /// Calculate total number of renderable lines
    pub(crate) fn calculate_total_lines(&mut self) {
        let mut total = 0;
        for (i, diff) in self.diffs.iter().enumerate() {
            // File header line
            total += 1;
            // If not collapsed, add hunk lines
            if !self.collapsed.contains(&i) {
                for hunk in &diff.hunks {
                    total += 1; // Hunk header
                    total += hunk.lines.len();
                }
            }
        }
        self.total_lines = total;
    }
}

/// Collect every file diff for the panel's target: a single commit or stash
/// when `commit_hash` is set, otherwise the working tree's unstaged and staged
/// changes. Runs on the worker thread, so it takes plain values rather than
/// borrowing the panel.
fn load_diffs(repo: &Path, commit_hash: Option<&str>, is_stash: bool) -> Vec<FileDiff> {
    if let Some(hash) = commit_hash {
        return load_commit_diff(repo, hash, is_stash);
    }

    let mut diffs = Vec::new();
    for file in git::get_unstaged_files(repo) {
        if let Some(diff) = load_file_diff(repo, &file.path, false, file.status) {
            diffs.push(diff);
        }
    }
    for file in git::get_staged_files(repo) {
        if let Some(diff) = load_file_diff(repo, &file.path, true, file.status) {
            diffs.push(diff);
        }
    }
    diffs
}

/// What waits for review of the agent of `session`, and the files both
/// the user and the agent edited.
fn load_review(repo: &Path, session: &str) -> (Vec<FileDiff>, Vec<String>) {
    let Some(review) = git::Review::open(repo, session) else {
        return (Vec::new(), Vec::new());
    };
    match review.diff() {
        Ok(Some(diff)) => (
            parse_patch(&diff.patch),
            diff.both
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        ),
        Ok(None) => (Vec::new(), Vec::new()),
        Err(error) => {
            log::warn!("cannot load the review of {session}: {error}");
            (Vec::new(), Vec::new())
        }
    }
}

/// Parse `git show` / `git stash show -p` output into per-file diffs.
fn load_commit_diff(repo: &Path, hash: &str, is_stash: bool) -> Vec<FileDiff> {
    let diff_output = if is_stash {
        git::stash_diff(repo, hash)
    } else {
        git::get_commit_diff(repo, hash)
    };
    diff_output.map_or_else(Vec::new, |output| parse_patch(&output))
}

/// Parse a patch of several files, after whatever heads it, into per-file
/// diffs.
fn parse_patch(diff_output: &str) -> Vec<FileDiff> {
    let mut diffs = Vec::new();

    // Parse git show output (contains multiple files)
    // Format:
    // commit <hash>
    // Author: <author>
    // Date:   <date>
    //
    //     <message>
    //
    // diff --git a/<file> b/<file>
    // --- a/<file>
    // +++ b/<file>
    // @@ ... @@
    // ...

    let mut current_file: Option<FileDiff> = None;
    let mut current_hunk: Option<DiffHunk> = None;
    let mut old_line = 0usize;
    let mut new_line = 0usize;

    for line in diff_output.lines() {
        // Start of a new file diff
        if line.starts_with("diff --git ") {
            // Save previous hunk if exists
            if let Some(hunk) = current_hunk.take() {
                if let Some(ref mut file) = current_file {
                    file.hunks.push(hunk);
                }
            }
            // Save previous file if exists
            if let Some(file) = current_file.take() {
                diffs.push(file);
            }

            // Parse file path from "diff --git a/<path> b/<path>"
            let path = line
                .strip_prefix("diff --git ")
                .and_then(|s| s.split_once(' '))
                .map(|(a, _)| a.strip_prefix("a/").unwrap_or(a))
                .unwrap_or("")
                .to_string();

            current_file = Some(FileDiff {
                path,
                status: FileStatus::Modified, // Default, will be updated
                staged: false,                // Not applicable for commits
                additions: 0,
                deletions: 0,
                hunks: Vec::new(),
            });
        } else if line.starts_with("new file mode") {
            if let Some(ref mut file) = current_file {
                file.status = FileStatus::Added;
            }
        } else if line.starts_with("deleted file mode") {
            if let Some(ref mut file) = current_file {
                file.status = FileStatus::Deleted;
            }
        } else if line.starts_with("rename from") || line.starts_with("rename to") {
            if let Some(ref mut file) = current_file {
                file.status = FileStatus::Renamed;
            }
        } else if line.starts_with("@@") {
            // Save previous hunk if exists
            if let Some(hunk) = current_hunk.take() {
                if let Some(ref mut file) = current_file {
                    file.hunks.push(hunk);
                }
            }

            let (old_start, new_start) = parse_hunk_header(line);
            old_line = old_start;
            new_line = new_start;

            current_hunk = Some(DiffHunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current_hunk {
            if let Some(rest) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Added,
                    content: rest.to_string(),
                    old_line: None,
                    new_line: Some(new_line),
                });
                if let Some(ref mut file) = current_file {
                    file.additions += 1;
                }
                new_line += 1;
            } else if let Some(rest) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Removed,
                    content: rest.to_string(),
                    old_line: Some(old_line),
                    new_line: None,
                });
                if let Some(ref mut file) = current_file {
                    file.deletions += 1;
                }
                old_line += 1;
            } else if let Some(rest) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine {
                    kind: LineKind::Context,
                    content: rest.to_string(),
                    old_line: Some(old_line),
                    new_line: Some(new_line),
                });
                old_line += 1;
                new_line += 1;
            }
        }
    }

    // Don't forget the last hunk and file
    if let Some(hunk) = current_hunk {
        if let Some(ref mut file) = current_file {
            file.hunks.push(hunk);
        }
    }
    if let Some(file) = current_file {
        diffs.push(file);
    }

    diffs
}

/// Load diff for a single file
fn load_file_diff(repo: &Path, path: &Path, staged: bool, status_char: char) -> Option<FileDiff> {
    let status = match status_char {
        'A' => FileStatus::Added,
        'D' => FileStatus::Deleted,
        'R' => FileStatus::Renamed,
        '?' => FileStatus::Untracked,
        _ => FileStatus::Modified,
    };

    let diff_text = git::get_file_diff(repo, path, staged)?;
    let stats = git::get_file_diff_stats(repo, path, staged);

    let hunks = parse_diff(&diff_text);

    Some(FileDiff {
        path: path.to_string_lossy().into_owned(),
        status,
        staged,
        additions: stats.additions,
        deletions: stats.deletions,
        hunks,
    })
}

/// Parse unified diff output into hunks
fn parse_diff(diff_text: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current_hunk: Option<DiffHunk> = None;
    let mut old_line = 0usize;
    let mut new_line = 0usize;

    for line in diff_text.lines() {
        if line.starts_with("@@") {
            // Save previous hunk if exists
            if let Some(hunk) = current_hunk.take() {
                hunks.push(hunk);
            }

            // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@
            let (old_start, new_start) = parse_hunk_header(line);
            old_line = old_start;
            new_line = new_start;

            current_hunk = Some(DiffHunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current_hunk {
            let (kind, content) = if let Some(rest) = line.strip_prefix('+') {
                let diff_line = DiffLine {
                    kind: LineKind::Added,
                    content: rest.to_string(),
                    old_line: None,
                    new_line: Some(new_line),
                };
                new_line += 1;
                (LineKind::Added, diff_line)
            } else if let Some(rest) = line.strip_prefix('-') {
                let diff_line = DiffLine {
                    kind: LineKind::Removed,
                    content: rest.to_string(),
                    old_line: Some(old_line),
                    new_line: None,
                };
                old_line += 1;
                (LineKind::Removed, diff_line)
            } else if let Some(rest) = line.strip_prefix(' ') {
                let diff_line = DiffLine {
                    kind: LineKind::Context,
                    content: rest.to_string(),
                    old_line: Some(old_line),
                    new_line: Some(new_line),
                };
                old_line += 1;
                new_line += 1;
                (LineKind::Context, diff_line)
            } else {
                // Line without prefix (shouldn't happen in unified diff)
                continue;
            };

            let _ = kind; // silence unused warning
            hunk.lines.push(content);
        }
    }

    // Don't forget the last hunk
    if let Some(hunk) = current_hunk {
        hunks.push(hunk);
    }

    hunks
}

/// Parse hunk header to get start line numbers
fn parse_hunk_header(header: &str) -> (usize, usize) {
    if let Some(caps) = HUNK_HEADER_RE.captures(header) {
        let old_start: usize = caps
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1);
        let new_start: usize = caps
            .get(2)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(1);
        return (old_start, new_start);
    }
    (1, 1)
}
